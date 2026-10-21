//! `keys list`: the agent keys, by name and last four characters, never the key, and when each
//! last arrived (the newest record whose agent is the key's id, or `null`).

use std::collections::BTreeMap;

use nullrouter_engine::journal::records;
use nullrouter_engine::keys::{self, BreakBehaviour, Keys};
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::{Live, View, ViewError};

/// A server answers `last_used` from its cached segment index; without one the view reads the
/// journal itself.
pub const NEEDS: &[&str] = &["keys.last_used"];

pub fn build(home: &OperatorHome, _args: &Value, live: &Live) -> Result<View, ViewError> {
    let list = Keys::load(&home.path().join(keys::FILE)).map_err(ViewError::failed)?;
    let served = live.ok("keys.last_used")?.map(|a| a["last_used"].clone());
    let own = if served.is_none() { records::last_arrived(home.path()) } else { BTreeMap::new() };
    let last_used = |id: &str| match &served {
        Some(map) => map[id].clone(),
        None => own.get(id).map_or(Value::Null, |t| json!(t)),
    };
    let rows: Vec<Value> = list
        .iter()
        .map(|k| {
            let mut row = json!({
                "id": k.id,
                "name": k.name,
                "harness": k.harness,
                "key": format!("…{}", k.last4),
                "created": k.created,
                "revoked": k.revoked,
                "break": k.break_behaviour.map(BreakBehaviour::as_str),
                "last_used": last_used(&k.id),
            });
            // Only when bound, so a list with no adapter keys reads as it did before slice 004.
            if let Some(a) = &k.adapter {
                row["adapter"] = json!(a.as_str());
            }
            row
        })
        .collect();
    Ok(View::new(json!(rows)))
}
