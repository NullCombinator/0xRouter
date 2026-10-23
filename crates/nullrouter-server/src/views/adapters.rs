//! `adapters list`: every harness with an adapter, hermes as built-in, then each third-party
//! harness in the store's index with its active version, its unacknowledged alerts and its
//! versions. A read creates nothing: with no `adapters/` directory there is nothing to list.
//!
//! `--json`: `[{"harness","built_in":true}, {"harness","built_in":false,"active","alerts",
//! "versions":[{"id","semver","state","rebuilding","rebuild_failed"}]}]`.

use nullrouter_adapters::alerts::{Alert, AlertLog};
use nullrouter_adapters::store::{Index, Store, VersionEntry};
use nullrouter_adapters::{BUILTIN, HarnessName};
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::{Live, View, ViewError};

pub const NEEDS: &[&str] = &[];

/// The index, empty while there is no `adapters/`.
fn read_index(home: &OperatorHome) -> Result<Index, ViewError> {
    if !home.path().join("adapters").is_dir() {
        return Ok(Index::default());
    }
    Store::open(home.path()).map_err(ViewError::failed)?.load_index().map_err(ViewError::failed)
}

pub fn version_json(v: &VersionEntry) -> Value {
    json!({"id": v.id, "semver": v.semver, "state": v.state,
           "rebuilding": v.rebuilding, "rebuild_failed": v.rebuild_failed})
}

pub fn build(home: &OperatorHome, _args: &Value, _live: &Live) -> Result<View, ViewError> {
    let index = read_index(home)?;
    let alerts: Vec<Alert> = if home.path().join("adapters").is_dir() {
        AlertLog::open(&Store::open(home.path()).map_err(ViewError::failed)?).list().map_err(ViewError::failed)?
    } else {
        Vec::new()
    };
    let unacked = |name: &HarnessName| alerts.iter().filter(|a| a.acked.is_none() && &a.harness == name).count();
    let mut rows: Vec<Value> = BUILTIN.iter().map(|h| json!({"harness": h, "built_in": true})).collect();
    for h in index.harnesses.iter().filter(|h| !h.name.is_builtin()) {
        rows.push(json!({"harness": h.name, "built_in": false, "active": h.active, "alerts": unacked(&h.name),
                         "versions": h.versions.iter().map(version_json).collect::<Vec<Value>>()}));
    }
    Ok(View::new(Value::Array(rows)))
}
