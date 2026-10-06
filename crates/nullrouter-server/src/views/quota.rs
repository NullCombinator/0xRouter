//! `quota [provider [name]]` and `quota history`: provider-reported quota per account, and an
//! account's poll history.

use std::time::SystemTime;

use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::clock;
use nullrouter_engine::quota::{history, poll};
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::{Live, View, ViewError, open_registry};

/// The latest polls are the running server's.
pub const NEEDS: &[&str] = &["quota.list"];

/// Queued poll entries and running tallies reach disk before the history is read.
pub const HISTORY_NEEDS: &[&str] = &["quota.checkpoint"];

/// `DATE`: RFC 3339, or `YYYY-MM-DD` (midnight UTC).
pub fn date(s: &str) -> Result<SystemTime, ViewError> {
    let full = if s.len() == 10 { format!("{s}T00:00:00Z") } else { s.to_owned() };
    clock::parse_rfc3339(&full)
        .ok_or_else(|| ViewError::failed(format!("{s:?} is not a date (use YYYY-MM-DD or 2026-10-03T14:00:00Z)")))
}

/// Arguments: `provider`, `name` (strings or null). `extra.offline` is true when no server
/// answered: the accounts are then listed from the files, with nothing polled.
pub fn build(home: &OperatorHome, args: &Value, live: &Live) -> Result<View, ViewError> {
    if let Some(a) = live.ok("quota.list")? {
        return Ok(View {
            json: json!(a["accounts"].as_array().cloned().unwrap_or_default()),
            extra: json!({"offline": false}),
        });
    }
    let (provider, name) = (args["provider"].as_str(), args["name"].as_str());
    let list = Accounts::load(&home.path().join(accounts::FILE)).map_err(ViewError::failed)?;
    let reg = open_registry(home)?.snapshot();
    let accounts: Vec<Value> = list
        .iter()
        .filter(|a| provider.is_none_or(|p| a.provider == p) && name.is_none_or(|n| a.name == n))
        .map(|a| {
            let reported = reg.provider(&a.provider).is_ok_and(|p| poll::reported(p, a).is_some());
            json!({
                "provider": a.provider,
                "name": a.name,
                "kind": if a.is_signin() { "signin" } else { "key" },
                "reported": reported,
                "interval_s": a.poll_interval().as_secs(),
                "latest": null,
                "last_failure": null,
            })
        })
        .collect();
    Ok(View { json: json!(accounts), extra: json!({"offline": true}) })
}

/// Arguments: `provider`, `name`, `since` (a date string or null), `limit` (a number or null).
pub fn history(home: &OperatorHome, args: &Value, live: &Live) -> Result<View, ViewError> {
    live.ok("quota.checkpoint")?;
    let (provider, name) = (args["provider"].as_str().unwrap_or_default(), args["name"].as_str().unwrap_or_default());
    let since = args["since"].as_str().map(date).transpose()?;
    let limit = args["limit"].as_u64().map(|n| n as usize);
    let entries = history::read(home.path(), provider, name, since, limit).map_err(ViewError::failed)?;
    let entries: Vec<Value> = entries.iter().filter_map(|e| serde_json::to_value(e).ok()).collect();
    Ok(View::new(json!(entries)))
}
