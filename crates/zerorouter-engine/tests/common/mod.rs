//! An engine over a scripted upstream with any number of providers and accounts, shared by
//! the retry, fallback, stay-warm and timeout tests.

// Each test binary uses only part of this module.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::{Answer, Failure, Piece, TextRequest};
use zerorouter_engine::keys::AgentId;
use zerorouter_engine::records::{AttemptKind, Outcome, RequestRecord};
use zerorouter_engine::state::Engine;
use zerorouter_engine::testkit::{MockUpstream, Step};
use zerorouter_registry::OperatorHome;
use zerorouter_wire::codec::request;

pub const SECRET: &str = "sk-mock-SENTINEL-0003";

pub struct Setup {
    pub _dir: tempfile::TempDir,
    pub engine: Arc<Engine>,
    pub mock: MockUpstream,
}

/// A provider `id` on the openai-chat wire at `/<id>/chat/completions` with model `m1`.
/// `endpoint` is extra TOML for the endpoint table (`retry`, `timeout_ms`, …).
pub fn chat_plugin(mock: &MockUpstream, id: &str, endpoint: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n{endpoint}\n[[models]]\nid = \"m1\"\n",
        mock.url(&format!("/{id}/chat/completions"))
    )
}

/// A provider `id` on the anthropic-messages wire at `/<id>/messages` with model `m1`.
pub fn messages_plugin(mock: &MockUpstream, id: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"anthropic-messages\"\nheaders = {{ \"anthropic-version\" = \"2023-06-01\" }}\nauth = {{ header = \"x-api-key\", scheme = \"raw\" }}\n[[models]]\nid = \"m1\"\n",
        mock.url(&format!("/{id}/messages"))
    )
}

/// `plugins`: `(id, TOML)`; `accounts`: `(provider, account name)` in operator order;
/// `config`: extra `config.toml` (unified models).
pub async fn setup(plugins: impl FnOnce(&MockUpstream) -> Vec<(&'static str, String)>, accounts: &[(&str, &str)], config: &str) -> Setup {
    let mock = MockUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), format!("allow_private_endpoints = true\n{config}")).unwrap();
    std::fs::create_dir(dir.path().join("plugins")).unwrap();
    for (id, toml) in plugins(&mock) {
        std::fs::write(dir.path().join(format!("plugins/{id}.toml")), toml).unwrap();
    }
    let mut file = String::from("schema = 1\n");
    for (provider, name) in accounts {
        file += &format!("[[account]]\nprovider = \"{provider}\"\nname = \"{name}\"\nsecret = \"{SECRET}-{provider}-{name}\"\n");
    }
    zerorouter_engine::files::write_private(&dir.path().join(zerorouter_engine::accounts::FILE), &file).unwrap();
    let (engine, report) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    Setup { _dir: dir, engine: Arc::new(engine), mock }
}

/// A unified model `u` over `members` (`(provider, model)`).
pub fn unified(members: &[(&str, &str)]) -> String {
    let m: Vec<String> = members.iter().map(|(p, m)| format!("{{ provider = \"{p}\", model = \"{m}\" }}")).collect();
    format!("[[unified_model]]\nname = \"u\"\nmembers = [{}]\n", m.join(", "))
}

pub fn chat_body(target: &str, stream: bool) -> Value {
    json!({"model": target, "stream": stream, "messages": [{"role": "user", "content": "hi"}]})
}

pub fn request(s: &Setup, client: &str, target: &str, body: Value, agent: &str, cancel: CancellationToken) -> TextRequest {
    let st = s.engine.snapshot();
    let client = st.style(client).unwrap().clone();
    let ir = request::decode(&client, &body).unwrap();
    let id = zerorouter_engine::records::new_id();
    s.engine.records.insert(RequestRecord::new(id.clone(), "2026-09-28T00:00:00Z".into(), client.id.clone()));
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    TextRequest {
        id,
        arrived: Instant::now(),
        client,
        body,
        ir,
        headers: HeaderMap::new(),
        agent: AgentId::new(agent, Some("sess-1")),
        target: target.into(),
        stream,
        cancel,
        media: None,
    }
}

/// Sends a whole openai-chat request for `target` as agent `ak_test`.
pub async fn send(s: &Setup, target: &str) -> (String, Result<Answer, Failure>) {
    send_as(s, target, "ak_test").await
}

pub async fn send_as(s: &Setup, target: &str, agent: &str) -> (String, Result<Answer, Failure>) {
    let req = request(s, "openai-chat", target, chat_body(target, false), agent, CancellationToken::new());
    let id = req.id.clone();
    (id, s.engine.text(s.engine.snapshot(), req).await)
}

pub fn ok() -> Step {
    Step::json(
        200,
        json!({"id": "x", "object": "chat.completion", "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 1}}),
    )
}

pub fn err(status: u16) -> Step {
    Step::json(status, json!({"error": {"message": format!("scripted {status}"), "type": "server_error"}}))
}

pub fn chat_chunks() -> Step {
    let chunk = |delta: Value, finish: Value| {
        (None, json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}))
    };
    Step::sse(
        &[
            chunk(json!({"role": "assistant", "content": "Hel"}), Value::Null),
            chunk(json!({"content": "lo"}), Value::Null),
            chunk(json!({}), json!("stop")),
            (None, json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [], "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}})),
        ],
        true,
    )
}

pub async fn drain(rx: &mut tokio::sync::mpsc::Receiver<Piece>) -> Vec<Piece> {
    let mut out = Vec::new();
    while let Some(p) = rx.recv().await {
        out.push(p);
    }
    out
}

pub async fn settled(s: &Setup, id: &str) -> RequestRecord {
    for _ in 0..400 {
        let r = s.engine.records.get(id).unwrap();
        if r.outcome != Outcome::InProgress {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("record {id} never finished");
}

/// `(provider, account, kind)` of every attempt, skips included.
pub fn trail(r: &RequestRecord) -> Vec<(String, Option<String>, AttemptKind)> {
    r.attempts.iter().map(|a| (a.provider.clone(), a.account.clone(), a.kind)).collect()
}

pub fn t(provider: &str, account: &str, kind: AttemptKind) -> (String, Option<String>, AttemptKind) {
    (provider.to_owned(), Some(account.to_owned()), kind)
}

/// The paths the mock received, in order.
pub fn paths(s: &Setup) -> Vec<String> {
    s.mock.received().into_iter().map(|r| r.path_and_query).collect()
}

/// The account each request carried, read back from its secret.
pub fn accounts_hit(s: &Setup) -> Vec<String> {
    s.mock
        .received()
        .iter()
        .map(|r| {
            let auth = r.headers.get("authorization").or_else(|| r.headers.get("x-api-key")).and_then(|v| v.to_str().ok()).unwrap_or("");
            auth.rsplit(&format!("{SECRET}-")[..]).next().unwrap_or("").to_owned()
        })
        .collect()
}
