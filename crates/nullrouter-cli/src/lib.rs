//! The parts of the `nullrouter` CLI that drive an interactive flow, as a library so tests can
//! run them in-process against a mock identity provider: the shipped binary resolves the
//! provider from the operator's registry and calls the same code.

pub mod signin;

use nullrouter_registry::OperatorHome;
use nullrouter_server::operator::{self, CallError};
use serde_json::json;

/// After a file was saved: asks the running server to reload it. `Ok` carries what to
/// print (`applied`, or `saved; applies at next start` with no server); `Err` the reason
/// a running server refused the files, which then keeps its previous state.
pub fn apply(home: &OperatorHome) -> Result<&'static str, String> {
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
