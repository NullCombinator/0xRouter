//! `plugins list [--community]`: bundled and installed plugins as loaded, then, with
//! `--community`, every community plugin with its fit status.

use nullrouter_registry::OperatorHome;
use nullrouter_registry::community;
use serde_json::{Value, json};

use super::{Live, View, ViewError, open_registry};

pub const NEEDS: &[&str] = &[];

/// Arguments: `community` (a boolean).
pub fn build(home: &OperatorHome, args: &Value, _live: &Live) -> Result<View, ViewError> {
    let handle = open_registry(home)?;
    let reg = handle.snapshot();
    let mut rows: Vec<Value> = reg
        .providers()
        .map(|p| json!({"id": p.id, "set": if p.is_bundled() { "bundled" } else { "user" }, "status": "loaded"}))
        .collect();
    let report = reg.report();
    rows.extend(report.unsupported.iter().map(|u| json!({"id": u.id, "set": "user", "status": "unsupported"})));
    rows.extend(report.skipped.iter().map(|s| json!({"id": s.id, "set": "user", "status": "invalid"})));
    if args["community"] == true {
        for p in community::community() {
            let status = match &p.verdict {
                _ if community::is_installed(p.id, handle.home()) => "installed",
                Ok(v) if v.fits() => "fits",
                Ok(_) => "unsupported",
                Err(_) => "invalid",
            };
            rows.push(json!({"id": p.id, "set": "community", "status": status}));
        }
    }
    Ok(View::new(json!(rows)))
}
