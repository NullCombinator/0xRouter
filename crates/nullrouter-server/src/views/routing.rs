//! `routing [target]`: every target's accounts as the next cold decision sees them. The answer is
//! the running server's; with none, the read fails with exit code 4.

use nullrouter_registry::OperatorHome;
use serde_json::Value;

use super::{Live, View, ViewError};
use crate::operator;

pub const NEEDS: &[&str] = &["routing.view"];

/// Arguments: `target` (a string or null).
pub fn build(home: &OperatorHome, _args: &Value, live: &Live) -> Result<View, ViewError> {
    match live.ok("routing.view")? {
        Some(answer) => Ok(View::new(answer.clone())),
        None => Err(ViewError::new(
            4,
            format!(
                "no server is running on {}; the routing view is the running server's (start it with `nullrouter serve`)",
                operator::socket_path(home).display()
            ),
        )),
    }
}
