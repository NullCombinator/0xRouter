//! `latency`: the last 24 hours per agent and per provider (spec 010, research R1, R6). A running
//! server answers `latency.summary`; without one the view reads the journal itself, and the two
//! give the same numbers.

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use nullrouter_engine::clock;
use nullrouter_engine::journal::summary::{self, Latency, Window};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::usage::at_of;
use super::{Live, View, ViewError};

pub const NEEDS: &[&str] = &["latency.summary"];

pub const WINDOW: &str = "last 24 h";

/// `[at - 24 h, at)` as RFC 3339, for the operator op.
pub fn op_window(args: &Value) -> (Value, Value) {
    match at_of(args).and_then(range) {
        Ok((from, to)) => (json!(clock::rfc3339(from.into())), json!(clock::rfc3339(to.into()))),
        Err(_) => (Value::Null, Value::Null),
    }
}

fn range(at: Timestamp) -> Result<(Timestamp, Timestamp), ViewError> {
    let from = at.checked_sub(SignedDuration::from_hours(24)).map_err(ViewError::failed)?;
    Ok((from, at))
}

/// Arguments: `at` (RFC 3339, default now).
pub fn build(home: &OperatorHome, args: &Value, live: &Live) -> Result<View, ViewError> {
    let (from, to) = range(at_of(args)?)?;
    let latency: Value = match live.ok("latency.summary")? {
        Some(a) => a["latency"].clone(),
        None => {
            let w = Window { from: Some(from.into()), to: to.into() };
            let own: Latency = summary::latency(home.path(), &w, &[], live.running);
            serde_json::to_value(own).map_err(ViewError::failed)?
        }
    };
    let names: std::collections::BTreeMap<String, String> = Keys::load(&home.path().join(keys::FILE))
        .map(|k| k.iter().map(|k| (k.id.clone(), k.name.clone())).collect())
        .unwrap_or_default();
    let name = |id: &str| names.get(id).map_or(Value::Null, |n| json!(n));
    let by_requests = |a: &(String, Value), b: &(String, Value)| {
        b.1["requests"].as_u64().cmp(&a.1["requests"].as_u64()).then_with(|| a.0.cmp(&b.0))
    };
    let rows = |map: &Value| -> Vec<(String, Value)> {
        let mut rows: Vec<(String, Value)> =
            map.as_object().into_iter().flatten().map(|(k, v)| (k.clone(), v.clone())).collect();
        rows.sort_by(by_requests);
        rows
    };
    let agents: Vec<Value> = rows(&latency["agents"])
        .into_iter()
        .map(|(id, mut v)| {
            v["name"] = name(&id);
            v["id"] = json!(id);
            v
        })
        .collect();
    let providers: Vec<Value> = rows(&latency["providers"])
        .into_iter()
        .map(|(id, mut v)| {
            let per_agent: Vec<Value> = rows(&v["agents"])
                .into_iter()
                .map(|(a, n)| json!({"id": a, "name": name(&a), "requests": n}))
                .collect();
            v["agents"] = json!(per_agent);
            v["id"] = json!(id);
            v
        })
        .collect();
    Ok(View::new(json!({
        "window": WINDOW,
        "at": clock::rfc3339(to.into()),
        "from": clock::rfc3339(from.into()),
        "to": clock::rfc3339(to.into()),
        "zone": TimeZone::system().iana_name().unwrap_or("local"),
        "agents": agents,
        "providers": providers,
        "warnings": [],
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_the_24_hours_before_at() {
        let home = tempfile::tempdir().unwrap();
        let mut live = Live::none();
        live.answers.insert(
            "latency.summary",
            Some(json!({"ok": true, "latency": {
                "agents": {
                    "ak_a": {"requests": 2, "overhead": {"p50": 5.0, "p95": 9.0, "n": 2}, "ttft": null,
                             "last": {"result": "resolved", "at": "2026-10-06T14:01:58Z"}},
                    "ak_b": {"requests": 3, "overhead": null, "ttft": null, "last": null}},
                "providers": {
                    "xai": {"requests": 1, "own_ttft": null, "agents": {"ak_a": 1}, "last": null},
                    "anthropic": {"requests": 4, "own_ttft": {"p50": 410.0, "p95": 1900.0, "n": 4},
                                  "agents": {"ak_b": 3, "ak_a": 1}, "last": null}}}})),
        );
        let args = json!({"at": "2026-10-06T14:02:11Z"});
        let v = build(&OperatorHome::new(home.path()), &args, &live).unwrap().json;
        assert_eq!(v["window"], "last 24 h");
        assert_eq!(v["from"], "2026-10-05T14:02:11Z");
        assert_eq!(v["to"], "2026-10-06T14:02:11Z");
        assert_eq!(v["at"], "2026-10-06T14:02:11Z");
        // Busiest first; a key the home doesn't have is nameless.
        let ids: Vec<&str> = v["agents"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["ak_b", "ak_a"]);
        assert_eq!(v["agents"][1]["name"], Value::Null);
        assert_eq!(v["agents"][1]["overhead"], json!({"p50": 5.0, "p95": 9.0, "n": 2}));
        assert_eq!(v["agents"][0]["ttft"], Value::Null, "no value is null, never 0");
        let providers: Vec<&str> =
            v["providers"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(providers, ["anthropic", "xai"]);
        assert_eq!(
            v["providers"][0]["agents"],
            json!([{"id": "ak_b", "name": null, "requests": 3}, {"id": "ak_a", "name": null, "requests": 1}])
        );
    }

    #[test]
    fn an_empty_window_has_no_rows() {
        let home = tempfile::tempdir().unwrap();
        let args = json!({"at": "2026-10-06T14:02:11Z"});
        let v = build(&OperatorHome::new(home.path()), &args, &Live::none()).unwrap().json;
        assert_eq!(v["agents"], json!([]));
        assert_eq!(v["providers"], json!([]));
    }
}
