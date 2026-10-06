//! `accounts list [--long]`: every account with its state, from the files and, when a server
//! runs, its in-memory states and cooldowns.

use nullrouter_engine::accounts::{Account, Accounts};
use nullrouter_engine::clock::{parse_rfc3339, rfc3339};
use nullrouter_engine::tokens::{PersistedState, TokenEntry, TokenStore};
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::{Live, View, ViewError};

/// Cooldowns and in-memory states live in the running server; without one, the files say what
/// is kept.
pub const NEEDS: &[&str] = &["accounts.state"];

/// An account's state as listed: the running server's when one answers, else what
/// `accounts.toml` and `tokens.toml` say (the out-of-service states are kept there).
pub struct Shown {
    /// `active`, `refreshing`, `needs_sign_in`, `refused`, `disabled`.
    state: String,
    since: Option<String>,
    reason: Option<String>,
    /// The state column.
    text: String,
}

/// `2026-10-03T14:02:11Z` → `2026-10-03 14:02` (UTC).
fn minute(t: &str) -> String {
    match (t.get(..10), t.get(11..16)) {
        (Some(d), Some(hm)) => format!("{d} {hm}"),
        _ => t.to_owned(),
    }
}

fn ago(secs: u64) -> String {
    if secs < 120 { format!("{secs} s") } else { format!("{} min", secs / 60) }
}

fn shown(a: &Account, live: &Value, stored: Option<&TokenEntry>) -> Shown {
    let row = live["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["provider"] == a.provider.as_str() && s["name"] == a.name.as_str());
    let text_of = |v: &Value| v.as_str().map(str::to_owned);
    let (state, since, reason, expires) = match row.filter(|r| r["state"].is_string()) {
        Some(r) => (
            text_of(&r["state"]).unwrap_or_default(),
            text_of(&r["state_since"]),
            text_of(&r["state_reason"]),
            text_of(&r["expires_at"]),
        ),
        None if a.disabled => ("disabled".into(), None, None, None),
        None if !a.is_signin() => ("active".into(), None, None, None),
        None => match stored {
            None => ("needs_sign_in".into(), None, Some("not signed in".into()), None),
            Some(e) => {
                let state = match e.state {
                    Some(PersistedState::NeedsSignIn) => "needs_sign_in",
                    Some(PersistedState::Refused) => "refused",
                    None => "active",
                };
                (state.into(), e.state_since.map(rfc3339), e.state_reason.clone(), Some(rfc3339(e.expires_at)))
            }
        },
    };
    let since_reason = |what: &str| {
        let mut t = what.to_owned();
        if let Some(s) = &since {
            t = format!("{t} since {}", minute(s));
        }
        match reason.as_deref().filter(|r| !r.is_empty()) {
            Some(r) => format!("{t} ({r})"),
            None => t,
        }
    };
    let text = match state.as_str() {
        "needs_sign_in" => since_reason("needs sign-in"),
        "refused" => since_reason("refused by provider"),
        "refreshing" => {
            let now = nullrouter_engine::clock::now();
            match expires.as_deref().and_then(parse_rfc3339).and_then(|t| now.duration_since(t).ok()) {
                Some(d) => format!("refreshing (token expired {} ago, retrying)", ago(d.as_secs())),
                None => "refreshing (retrying)".into(),
            }
        }
        "active" => cooling(row),
        other => other.to_owned(),
    };
    Shown { state, since, reason, text }
}

/// `active`, or the running server's rests: `cooling <model> 12 s, …`.
fn cooling(row: Option<&Value>) -> String {
    let rests: Vec<String> = row
        .and_then(|r| r["cooling"].as_array())
        .into_iter()
        .flatten()
        .map(|c| {
            format!(
                "{} {} s",
                c["model"].as_str().unwrap_or("?"),
                c["remaining_ms"].as_u64().unwrap_or(0).div_ceil(1000)
            )
        })
        .collect();
    if rests.is_empty() { "active".into() } else { format!("cooling {}", rests.join(", ")) }
}

/// Arguments: `provider` (a string, or null).
pub fn build(home: &OperatorHome, args: &Value, live: &Live) -> Result<View, ViewError> {
    let list = Accounts::load(&home.path().join(nullrouter_engine::accounts::FILE)).map_err(ViewError::failed)?;
    let provider = args["provider"].as_str();
    // A server's refusal or a socket error is not fatal here: the files answer.
    let live = live.answer("accounts.state").cloned().unwrap_or(Value::Null);
    // Sign-in accounts show their access token's last four, as keys do (research R5).
    let mut warnings = Vec::new();
    let tokens = TokenStore::load(home.path()).unwrap_or_else(|e| {
        warnings.push(format!("warning: {e}"));
        TokenStore::default()
    });
    let rows: Vec<Value> = list
        .iter()
        .filter(|a| provider.is_none_or(|p| a.provider == p))
        .map(|a| {
            let t = tokens.get(&a.provider, &a.name).filter(|_| a.is_signin());
            let secret = match t {
                Some(t) => t.shown_token(),
                None if a.is_signin() => "…".to_owned(),
                None => a.shown_secret(),
            };
            let s = shown(a, &live, t);
            json!({
                "provider": a.provider,
                "name": a.name,
                "kind": if a.is_signin() { "signin" } else { "key" },
                "order": a.order,
                "priority": a.priority,
                "secret": secret,
                "state": s.state,
                "state_since": s.since,
                "state_reason": s.reason,
                "state_text": s.text,
                "email": t.and_then(|t| t.claims.email.clone()),
                "tier": t.and_then(|t| t.claims.tier.clone()),
                "expires_at": t.map(|t| rfc3339(t.expires_at)),
                "last_refresh_at": t.and_then(|t| t.last_refresh_at.map(rfc3339)),
            })
        })
        .collect();
    Ok(View { json: json!(rows), extra: json!({ "warnings": warnings }) })
}
