//! Live model lists (`[models_live]`, research R14): read with a provider's first active
//! sign-in account at start and every `refresh` (6 h), kept in memory, and listed beside the
//! static `[[models]]`, which stay the list whenever the live one can't be read.
//!
//! The extractor mirrors 9router's `parseGrokCliModels` (`open-sse/services/grokCliModels.js`):
//! the list is the first of the declared paths that holds an array (an object at a named path
//! is read as `id → entry`); an entry that is a string is its own id; ids are trimmed and
//! deduplicated; names fall back to the id; context and output limits count only when
//! positive, and fall back to the static model's.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use nullrouter_registry::ProviderEntity;
use nullrouter_registry::schema::{LiveModelType, ModelType, ModelsLiveDecl};
use serde_json::Value;

use crate::accounts;
use crate::quota::extract::{number_at, text_at, value_at};
use crate::quota::poll::{AccountCall, PollError};
use crate::state::Engine;

/// One listed model.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveModel {
    pub id: String,
    pub name: String,
    pub context: Option<u64>,
    pub max_output: Option<u64>,
    pub ty: ModelType,
}

fn positive(v: Option<f64>) -> Option<u64> {
    v.filter(|n| n.is_finite() && *n > 0.0).map(|n| n as u64)
}

/// The models `body` lists under `decl`'s paths, in order.
pub fn parse(decl: &ModelsLiveDecl, body: &Value) -> Vec<LiveModel> {
    let entries: Vec<(Option<&str>, &Value)> = decl
        .list
        .alternatives()
        .find_map(|alt| {
            let v = if alt == "." { Some(body) } else { body.get(alt).filter(|v| !v.is_null()) }?;
            match v {
                Value::Array(items) => Some(items.iter().map(|i| (None, i)).collect()),
                Value::Object(map) if alt != "." => Some(map.iter().map(|(k, i)| (Some(k.as_str()), i)).collect()),
                _ => None,
            }
        })
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (key, raw) in entries {
        let item = match raw {
            Value::String(s) => &Value::Object([("id".to_owned(), Value::String(s.clone()))].into_iter().collect()),
            Value::Object(_) => raw,
            _ => continue,
        };
        let Some(id) = text_at(item, &decl.id).or_else(|| key.map(str::to_owned)).filter(|s| !s.is_empty()) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let name = decl.name.as_ref().and_then(|p| text_at(item, p)).unwrap_or_else(|| id.clone());
        let ty = match &decl.model_type {
            LiveModelType::Fixed(t) => *t,
            LiveModelType::Path(p) => {
                value_at(item, p).and_then(Value::as_str).and_then(ModelType::parse).unwrap_or(ModelType::Text)
            }
        };
        out.push(LiveModel {
            name,
            context: positive(decl.context.as_ref().and_then(|p| number_at(item, p))),
            max_output: positive(decl.max_output.as_ref().and_then(|p| number_at(item, p))),
            ty,
            id,
        });
    }
    out
}

/// `live` with each model's missing limits taken from the static `[[models]]` entry of the
/// same id (9router fills grok-build's from its constants).
pub fn with_static_limits(provider: &ProviderEntity, live: Vec<LiveModel>) -> Vec<LiveModel> {
    live.into_iter()
        .map(|mut m| {
            if let Some(s) = provider.models.iter().flatten().find(|s| s.id == m.id) {
                m.context = m.context.or(s.context_length);
                m.max_output = m.max_output.or(s.max_output_tokens);
            }
            m
        })
        .collect()
}

#[derive(Debug, Clone)]
struct List {
    models: Vec<LiveModel>,
    tried_at: SystemTime,
    retry_pending: bool,
}

/// The live lists by provider, shared by every snapshot.
#[derive(Debug, Default)]
pub struct LiveModels {
    lists: Mutex<HashMap<String, List>>,
}

impl LiveModels {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, List>> {
        self.lists.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The provider's last good live list (empty when never read).
    pub fn models(&self, provider: &str) -> Vec<LiveModel> {
        self.lock().get(provider).map(|l| l.models.clone()).unwrap_or_default()
    }

    /// The model `id` in the provider's last good live list.
    pub fn find(&self, provider: &str, id: &str) -> Option<LiveModel> {
        self.lock().get(provider).and_then(|l| l.models.iter().find(|m| m.id == id).cloned())
    }

    /// Whether the provider's last good live list holds `id`.
    pub fn has(&self, provider: &str, id: &str) -> bool {
        self.lock().get(provider).is_some_and(|l| l.models.iter().any(|m| m.id == id))
    }

    /// When the provider's list is next read: now when never tried; `retry_after` after a
    /// failure not yet retried; else `refresh` after the last try.
    pub fn due(&self, provider: &str, refresh: Duration, retry_after: Duration) -> SystemTime {
        match self.lock().get(provider) {
            None => UNIX_EPOCH,
            Some(l) if l.retry_pending => l.tried_at + retry_after,
            Some(l) => l.tried_at + refresh,
        }
    }

    /// Keeps a read's outcome. A failed read keeps the last good list (the static list
    /// alone when there was none).
    fn record(&self, provider: &str, result: Result<Vec<LiveModel>, ()>) {
        let mut lists = self.lock();
        let prev = lists.remove(provider);
        let was_retry = prev.as_ref().is_some_and(|l| l.retry_pending);
        let next = match result {
            Ok(models) => List { models, tried_at: SystemTime::now(), retry_pending: false },
            Err(()) => List {
                models: prev.map(|l| l.models).unwrap_or_default(),
                tried_at: SystemTime::now(),
                retry_pending: !was_retry,
            },
        };
        lists.insert(provider.to_owned(), next);
    }
}

impl Engine {
    /// Reads `provider`'s live model list with its first active sign-in account and keeps
    /// it. Returns how many models it listed.
    pub async fn fetch_live_models(self: &Arc<Self>, provider: &str) -> Result<usize, PollError> {
        let st = self.snapshot();
        let no_account = || PollError {
            class: crate::quota::poll::PollErrorClass::Withheld,
            status: None,
            reason: format!("{provider} has no active sign-in account"),
        };
        let entity = st.registry.provider(provider).map_err(|_| no_account())?;
        let Some(decl) = &entity.models_live else { return Ok(0) };
        let account = first_signed_in(&st, entity).ok_or_else(no_account)?;
        let call = AccountCall { url: &decl.url, method: "GET", headers: &decl.headers, body: Bytes::new() };
        let result = self.account_call(&st, entity, account, call).await.and_then(|body| {
            let v: Value = serde_json::from_slice(&body).map_err(|e| PollError {
                class: crate::quota::poll::PollErrorClass::Status,
                status: None,
                reason: format!("not JSON: {e}"),
            })?;
            Ok(with_static_limits(entity, parse(decl, &v)))
        });
        match result {
            Ok(models) => {
                let n = models.len();
                st.live_models.record(provider, Ok(models));
                tracing::debug!(provider, models = n, "live model list read");
                Ok(n)
            }
            Err(e) => {
                st.live_models.record(provider, Err(()));
                tracing::warn!(provider, "live model list not read ({}); the static list stays", e.summary());
                Err(e)
            }
        }
    }
}

/// The provider's first enabled sign-in account, in order, that can serve now.
pub fn first_signed_in<'a>(st: &'a crate::state::EngineState, p: &'a ProviderEntity) -> Option<&'a accounts::Account> {
    st.accounts.for_provider(&p.id).find(|a| a.is_signin() && accounts::out_of_service(a, &st.tokens).is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decl() -> ModelsLiveDecl {
        toml::from_str(
            r#"
url = "https://a.example/v1/models"
list = "data | models | results | ."
id = "id | model_id | modelId | model | slug | name"
name = "display_name | displayName | name"
context = "context_length | contextLength | context_window | contextWindow"
max_output = "max_output_tokens | maxOutputTokens"
"#,
        )
        .unwrap()
    }

    #[test]
    fn reads_each_shape() {
        let d = decl();
        let ids = |b: Value| parse(&d, &b).into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(json!({"data": [{"id": "a"}, {"id": "a"}, {"id": " b "}]})), ["a", "b"]);
        assert_eq!(ids(json!([{"model_id": "x"}, "y", 3])), ["x", "y"]);
        assert_eq!(ids(json!({"models": {"k1": {"display_name": "K"}, "k2": "k2-id"}})), ["k1", "k2-id"]);
        assert!(ids(json!({})).is_empty());
        assert!(ids(json!({"error": "nope"})).is_empty(), "a root object is no list");
        let m = &parse(
            &d,
            &json!({"data": [{"id": "a", "display_name": "A", "context_window": "1000", "max_output_tokens": 0}]}),
        )[0];
        assert_eq!((m.name.as_str(), m.context, m.max_output, m.ty), ("A", Some(1000), None, ModelType::Text));
    }

    #[test]
    fn a_failed_read_keeps_the_last_good_list_and_retries_once() {
        let l = LiveModels::default();
        let (refresh, retry) = (Duration::from_secs(3600), Duration::from_secs(60));
        assert_eq!(l.due("p", refresh, retry), UNIX_EPOCH);
        let m = LiveModel { id: "a".into(), name: "a".into(), context: None, max_output: None, ty: ModelType::Text };
        l.record("p", Ok(vec![m]));
        l.record("p", Err(()));
        assert_eq!(l.models("p").len(), 1);
        let first = l.due("p", refresh, retry);
        l.record("p", Err(()));
        let second = l.due("p", refresh, retry);
        assert!(second > first + Duration::from_secs(3000), "after the one retry: the refresh period");
    }
}
