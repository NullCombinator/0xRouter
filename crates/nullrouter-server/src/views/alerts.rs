//! `alerts list [--all]`: the adapter alerts in `adapters/alerts.toml`, newest first;
//! unacknowledged ones only unless `all` is true. A read creates nothing.

use nullrouter_adapters::alerts::{Alert, AlertLog};
use nullrouter_adapters::store::Store;
use nullrouter_registry::OperatorHome;
use serde_json::Value;

use super::{Live, View, ViewError};

pub const NEEDS: &[&str] = &[];

/// Arguments: `all` (a boolean).
pub fn build(home: &OperatorHome, args: &Value, _live: &Live) -> Result<View, ViewError> {
    let all = args["all"].as_bool().unwrap_or(false);
    let stored = if home.path().join("adapters").is_dir() {
        AlertLog::open(&Store::open(home.path()).map_err(ViewError::failed)?).list().map_err(ViewError::failed)?
    } else {
        Vec::new()
    };
    // The log is kept oldest first; the listing is newest first.
    let shown: Vec<&Alert> = stored.iter().rev().filter(|a| all || a.acked.is_none()).collect();
    Ok(View::new(serde_json::to_value(&shown).map_err(ViewError::failed)?))
}
