//! The operator socket's quota ops (slice 005 contracts/operator-cli.md § Operator socket):
//!
//! | Request | Response |
//! |---|---|
//! | `{"op":"quota.list","provider"?,"name"?}` | `{"ok":true,"accounts":[…]}`: per account `kind`, `reported`, `interval_s`, `latest`, `last_failure` |
//! | `{"op":"quota.poll","provider","name"}` | `{"ok":true,"poll":{…}}`: the poll just run |
//! | `{"op":"quota.checkpoint"}` | `{"ok":true,"written":N}`: queued poll entries and running tallies flushed to disk |
//! | `{"op":"quota.outside","provider"?,"account"?,"since"?,"limit"?}` | `{"ok":true,"entries":[…]}`: each an `OutsideEntry` plus `provider` and `account` |
//! | `{"op":"quota.alerts"}` | `{"ok":true,"alerts":[…]}`: every account's unacknowledged `Alert`s, each plus `provider` and `account` |
//! | `{"op":"quota.ack","id"?,"provider"?,"account"?}` | `{"ok":true,"acknowledged":N}`: no `id` acknowledges every open alert (narrowed); an `id` or unique prefix one |
//!
//! A poll is `{provider, account, at, windows, error?, retry}`; a failed one's `error` carries
//! `class`, `status?`, a redacted `reason` and the CLI's `summary`. Tokens never cross.

use std::sync::Arc;

use nullrouter_engine::clock;
use nullrouter_engine::files::FileError;
use nullrouter_engine::quota::fit::outside::{self, AckTarget};
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

/// The `(provider, name)` of each account in the current state that `provider` and `account`
/// (by name) keep.
fn accounts_of(engine: &Engine, provider: Option<&str>, account: Option<&str>) -> Vec<(String, String)> {
    let st = engine.snapshot();
    st.accounts
        .iter()
        .filter(|a| provider.is_none_or(|p| a.provider == p) && account.is_none_or(|n| a.name == n))
        .map(|a| (a.provider.clone(), a.name.clone()))
        .collect()
}

/// `value` (a JSON object) with `provider` and `account` added.
fn tagged(mut value: Value, provider: &str, account: &str) -> Value {
    if let Some(obj) = value.as_object_mut() {
        obj.insert("provider".into(), json!(provider));
        obj.insert("account".into(), json!(account));
    }
    value
}

/// `quota.outside`: the outside-use entries of the accounts `provider` and `account` narrow,
/// each account's newest first. `since` (RFC 3339) and `limit` apply to each account as the
/// engine reads it. File reads run off the async threads.
pub async fn outside(engine: &Arc<Engine>, req: &Value) -> Value {
    let provider = req.get("provider").and_then(Value::as_str);
    let account = req.get("account").and_then(Value::as_str);
    let since = match req.get("since").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => match v.as_str().and_then(clock::parse_rfc3339) {
            Some(t) => Some(t),
            None => return json!({"ok": false, "error": "since is an RFC 3339 time"}),
        },
    };
    let limit = match req.get("limit").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => match v.as_u64() {
            Some(n) => Some(n as usize),
            None => return json!({"ok": false, "error": "limit is a count"}),
        },
    };
    let accounts = accounts_of(engine, provider, account);
    let home = engine.home().path().to_owned();
    let read = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, FileError> {
        let mut entries = Vec::new();
        for (p, a) in &accounts {
            for e in outside::read(&home, p, a, since, limit)? {
                entries.push(tagged(serde_json::to_value(&e).unwrap_or(Value::Null), p, a));
            }
        }
        Ok(entries)
    })
    .await;
    match read {
        Ok(Ok(entries)) => json!({"ok": true, "entries": entries}),
        Ok(Err(e)) => json!({"ok": false, "error": e.to_string()}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

/// `quota.alerts`: every account's unacknowledged alerts, newest first per account, each with
/// `provider` and `account`.
pub async fn alerts(engine: &Arc<Engine>) -> Value {
    let accounts = accounts_of(engine, None, None);
    let home = engine.home().path().to_owned();
    let raised = tokio::task::spawn_blocking(move || {
        let mut alerts = Vec::new();
        for (p, a) in &accounts {
            for al in outside::alerts(&home, p, a) {
                alerts.push(tagged(serde_json::to_value(&al).unwrap_or(Value::Null), p, a));
            }
        }
        alerts
    })
    .await;
    match raised {
        Ok(alerts) => json!({"ok": true, "alerts": alerts}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

/// `quota.ack`: acknowledges the open alerts `id` names (a full id or a unique prefix), or every
/// open alert when there is no `id`, in the accounts `provider` and `account` narrow. Answers the
/// number acknowledged; an engine error (such as an ambiguous prefix) is `ok: false`.
pub async fn ack(engine: &Arc<Engine>, req: &Value) -> Value {
    let id = req.get("id").and_then(Value::as_str).map(str::to_owned);
    let provider = req.get("provider").and_then(Value::as_str);
    let account = req.get("account").and_then(Value::as_str);
    let accounts = accounts_of(engine, provider, account);
    let home = engine.home().path().to_owned();
    let now = clock::now();
    let done = tokio::task::spawn_blocking(move || -> Result<usize, FileError> {
        let mut n = 0;
        for (p, a) in &accounts {
            let target = match &id {
                Some(id) => AckTarget::Id(id.as_str()),
                None => AckTarget::All,
            };
            n += outside::ack(&home, p, a, target, now)?.len();
        }
        Ok(n)
    })
    .await;
    match done {
        Ok(Ok(n)) => json!({"ok": true, "acknowledged": n}),
        Ok(Err(e)) => json!({"ok": false, "error": e.to_string()}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}
