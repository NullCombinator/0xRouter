//! `records list` and `records show`: the request records, read from the journal's segments on
//! disk by both routes (spec 008 Clarifications Q1), with a running server's copy of an
//! unfinished request.

use std::time::SystemTime;

use nullrouter_engine::clock;
use nullrouter_engine::journal::records;
use nullrouter_engine::keys::{self, Keys};
use nullrouter_registry::OperatorHome;
use serde_json::{Map, Value, json};

use super::{Live, View, ViewError};

/// The list reads the disk only.
pub const NEEDS: &[&str] = &[];

/// A running server knows the freshest copy of an unfinished request.
pub const RECORD_NEEDS: &[&str] = &["records.get"];

/// `2026-10-04` or an RFC 3339 time.
pub fn parse_when(text: &str) -> Result<SystemTime, ViewError> {
    let full = if text.len() == 10 { format!("{text}T00:00:00Z") } else { text.to_owned() };
    clock::parse_rfc3339(&full)
        .ok_or_else(|| ViewError::failed(format!("{text:?} is not a date; use 2026-10-04 or 2026-10-04T09:00:00Z")))
}

/// What a request with no `close` is called: in flight with a server, cut short without one.
fn settle_open(records: &mut [Value], running: bool) {
    for r in records {
        if r["outcome"] == "in_progress" && r["job"].is_null() && !running {
            r["outcome"] = json!("interrupted");
        }
    }
}

fn text(args: &Value, key: &str) -> Option<String> {
    args[key].as_str().map(str::to_owned)
}

/// Arguments: the filter (`provider`, `account`, `agent`, `model`, `reason`, `since`, `limit`),
/// each a string, a number or null.
pub fn build(home: &OperatorHome, args: &Value, live: &Live) -> Result<View, ViewError> {
    let since = args["since"].as_str().map(parse_when).transpose()?;
    let filter = records::Filter {
        provider: text(args, "provider"),
        account: text(args, "account"),
        agent: text(args, "agent"),
        model: text(args, "model"),
        reason: text(args, "reason"),
        since,
        limit: args["limit"].as_u64().map(|n| n as usize),
        before: text(args, "before"),
        test: args["test"].as_bool(),
        unified_model: None,
    };
    // The page back starts at a record that exists.
    if let Some(id) = &filter.before
        && !records::cursor_exists(home.path(), id)
    {
        return Err(ViewError::failed(format!("no record {id}")));
    }
    let mut found = records::read(home.path(), &filter);
    settle_open(&mut found, live.running);
    Ok(View::new(Value::Array(found)))
}

/// Arguments: `id`. `extra.key_names` maps agent key ids to the names `keys.toml` gives them,
/// which the text shows and the record's JSON (the id only) does not.
pub fn record(home: &OperatorHome, args: &Value, live: &Live) -> Result<View, ViewError> {
    let id = args["id"].as_str().unwrap_or_default();
    // Records name keys by id; the operator knows them by name.
    let names: Map<String, Value> = Keys::load(&home.path().join(keys::FILE))
        .map(|k| k.iter().map(|k| (k.id.clone(), json!(k.name))).collect())
        .unwrap_or_default();
    let fresh = live.answer("records.get").filter(|a| a["ok"] == true).map(|a| a["record"].clone());
    let mut found = fresh.or_else(|| records::get(home.path(), id)).into_iter().collect::<Vec<_>>();
    settle_open(&mut found, live.running);
    let Some(record) = found.pop() else { return Err(ViewError::failed(format!("no record {id}"))) };
    Ok(View { json: record, extra: json!({ "key_names": names }) })
}
