//! The operator socket's quota ops (slice 005 contracts/operator-cli.md § Operator socket):
//!
//! | Request | Response |
//! |---|---|
//! | `{"op":"quota.list","provider"?,"name"?}` | `{"ok":true,"accounts":[…]}`: per account `kind`, `reported`, `interval_s`, `latest`, `last_failure` |
//! | `{"op":"quota.poll","provider","name"}` | `{"ok":true,"poll":{…}}`: the poll just run |
//! | `{"op":"quota.checkpoint"}` | `{"ok":true,"written":N}`: queued poll entries and running tallies flushed to disk |
//!
//! A poll is `{provider, account, at, windows, error?, retry}`; a failed one's `error` carries
//! `class`, `status?`, a redacted `reason` and the CLI's `summary`. Tokens never cross.

use std::sync::Arc;

use nullrouter_engine::quota::poll::{self, QuotaPoll};
use nullrouter_engine::state::Engine;
use serde_json::{Value, json};

fn poll_json(p: &QuotaPoll) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
    if let (Some(e), Some(obj)) = (&p.error, v.get_mut("error").and_then(Value::as_object_mut)) {
        obj.insert("summary".into(), json!(e.summary()));
    }
    v
}

/// `quota.list`: every account (of `provider`, and named `name`, when given), in
/// `accounts.toml` order.
pub fn list(engine: &Arc<Engine>, req: &Value) -> Value {
    let provider = req.get("provider").and_then(Value::as_str);
    let name = req.get("name").and_then(Value::as_str);
    let st = engine.snapshot();
    let accounts: Vec<Value> = st
        .accounts
        .iter()
        .filter(|a| provider.is_none_or(|p| a.provider == p) && name.is_none_or(|n| a.name == n))
        .map(|a| {
            let reported = st.registry.provider(&a.provider).is_ok_and(|p| poll::reported(p, a).is_some());
            let q = engine.quota.get(&a.provider, &a.name);
            json!({
                "provider": a.provider,
                "name": a.name,
                "kind": if a.is_signin() { "signin" } else { "key" },
                "disabled": a.disabled,
                "reported": reported,
                "interval_s": a.poll_interval().as_secs(),
                "latest": q.latest.as_ref().map(poll_json),
                "last_failure": q.last_failure.as_ref().map(poll_json),
            })
        })
        .collect();
    json!({"ok": true, "accounts": accounts})
}

/// `quota.poll`: polls one account now.
pub async fn poll_now(engine: &Arc<Engine>, req: &Value) -> Value {
    let (Some(provider), Some(name)) =
        (req.get("provider").and_then(Value::as_str), req.get("name").and_then(Value::as_str))
    else {
        return json!({"ok": false, "error": "quota.poll needs provider and name"});
    };
    let st = engine.snapshot();
    let Some(account) = st.accounts.get(provider, name) else {
        return json!({"ok": false, "error": format!("provider {provider} has no account named {name}")});
    };
    if !st.registry.provider(provider).is_ok_and(|p| poll::reported(p, account).is_some()) {
        return json!({"ok": false, "error": format!("{provider}/{name}: quota not reported")});
    }
    match engine.poll_quota(provider, name).await {
        Some(p) => json!({"ok": true, "poll": poll_json(&p)}),
        None => json!({"ok": false, "error": format!("{provider}/{name}: quota not reported")}),
    }
}

/// `quota.checkpoint`: writes queued poll entries and checkpoints changed tallies, so the
/// CLI can work on the history files (`quota history`, `prune`, `forget`).
pub async fn checkpoint(engine: &Arc<Engine>) -> Value {
    json!({"ok": true, "written": engine.checkpoint_tallies().await})
}
