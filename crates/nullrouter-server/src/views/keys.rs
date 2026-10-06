//! `keys list`: the agent keys, by name and last four characters, never the key.

use nullrouter_engine::keys::{self, BreakBehaviour, Keys};
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::{Live, View, ViewError};

/// No live ops: `keys.toml` is the whole answer.
pub const NEEDS: &[&str] = &[];

pub fn build(home: &OperatorHome, _args: &Value, _live: &Live) -> Result<View, ViewError> {
    let list = Keys::load(&home.path().join(keys::FILE)).map_err(ViewError::failed)?;
    let rows: Vec<Value> = list
        .iter()
        .map(|k| {
            json!({
                "id": k.id,
                "name": k.name,
                "key": format!("…{}", k.last4),
                "created": k.created,
                "revoked": k.revoked,
                "break": k.break_behaviour.map(BreakBehaviour::as_str),
            })
        })
        .collect();
    Ok(View::new(json!(rows)))
}
