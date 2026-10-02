pub(crate) mod accounts;
pub(crate) mod behaviour;
pub(crate) mod check;
pub(crate) mod keys;
pub(crate) mod model;
pub(crate) mod plugins;
pub(crate) mod providers;
pub(crate) mod records;
pub(crate) mod resolve;
pub(crate) mod serve;
pub(crate) mod validate;

use nullrouter_registry::OperatorHome;
use nullrouter_server::operator::{self, CallError};
use serde_json::json;

/// After a file was saved: asks the running server to reload it. `Ok` carries what to
/// print (`applied`, or `saved; applies at next start` with no server); `Err` the reason
/// a running server refused the files, which then keeps its previous state.
pub(crate) fn apply(home: &OperatorHome) -> Result<&'static str, String> {
    match operator::call(home, &json!({"op": "reload"})) {
        Ok(a) if a["ok"] == true => Ok("applied"),
        Ok(a) => Err(format!(
            "saved, but the running server kept its previous state: {}",
            a["error"].as_str().unwrap_or("unknown error")
        )),
        Err(CallError::NoServer(_)) => Ok("saved; applies at next start"),
        Err(e) => Err(format!("saved, but the running server wasn't told: {e}")),
    }
}
