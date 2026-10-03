//! An engine over a scripted upstream with any number of providers and accounts, shared by
//! the retry, fallback, stay-warm and timeout tests.

// Each test binary uses only part of this module.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use nullrouter_engine::attempt::{Answer, Failure, Piece, TextRequest};
use nullrouter_engine::keys::AgentId;
use nullrouter_engine::records::{AttemptKind, Outcome, RequestRecord};
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_registry::OperatorHome;
use nullrouter_wire::codec::request;
use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

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
pub async fn setup(
    plugins: impl FnOnce(&MockUpstream) -> Vec<(&'static str, String)>,
    accounts: &[(&str, &str)],
    config: &str,
) -> Setup {
    let mock = MockUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), format!("allow_private_endpoints = true\n{config}")).unwrap();
    std::fs::create_dir(dir.path().join("plugins")).unwrap();
    for (id, toml) in plugins(&mock) {
        std::fs::write(dir.path().join(format!("plugins/{id}.toml")), toml).unwrap();
    }
    let mut file = String::from("schema = 1\n");
    for (provider, name) in accounts {
        file += &format!(
            "[[account]]\nprovider = \"{provider}\"\nname = \"{name}\"\nsecret = \"{SECRET}-{provider}-{name}\"\n"
        );
    }
    nullrouter_engine::files::write_private(&dir.path().join(nullrouter_engine::accounts::FILE), &file).unwrap();
    let (engine, report) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    Setup { _dir: dir, engine: Arc::new(engine), mock }
}

/// [`setup`] where `signin` names the `(provider, account)` pairs that are sign-in accounts:
/// their access token is the same `{SECRET}-<provider>-<name>` a key account would carry
/// (so [`accounts_hit`] reads both), kept in `tokens.toml` and bound to the mock's host,
/// with an email and user id as claims. The engine opens over the parity set, so these
/// user plugins may declare `[signin]` and `[identity]`.
pub async fn setup_signin(
    plugins: impl FnOnce(&MockUpstream) -> Vec<(&'static str, String)>,
    accounts: &[(&str, &str)],
    signin: &[(&str, &str)],
    config: &str,
) -> Setup {
    let mock = MockUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), format!("allow_private_endpoints = true\n{config}")).unwrap();
    std::fs::create_dir(dir.path().join("plugins")).unwrap();
    for (id, toml) in plugins(&mock) {
        std::fs::write(dir.path().join(format!("plugins/{id}.toml")), toml).unwrap();
    }
    let (mut file, mut tokens) = (String::from("schema = 2\n"), String::from("schema = 1\n"));
    for (order, (provider, name)) in accounts.iter().enumerate() {
        let token = format!("{SECRET}-{provider}-{name}");
        if signin.contains(&(provider, name)) {
            file += &format!(
                "[[account]]\nprovider = \"{provider}\"\nname = \"{name}\"\nkind = \"signin\"\norder = {order}\n"
            );
            tokens += &format!(
                "[[token]]\nprovider = \"{provider}\"\nname = \"{name}\"\naccess_token = \"{token}\"\nexpires_at = \"2099-01-01T00:00:00Z\"\nsigned_in_at = \"2026-10-01T00:00:00Z\"\nhosts = [\"127.0.0.1\"]\nclaims = {{ email = \"{name}@example.com\", user_id = \"user-{name}\" }}\n"
            );
        } else {
            file += &format!(
                "[[account]]\nprovider = \"{provider}\"\nname = \"{name}\"\nsecret = \"{token}\"\norder = {order}\n"
            );
        }
    }
    nullrouter_engine::files::write_private(&dir.path().join(nullrouter_engine::accounts::FILE), &file).unwrap();
    nullrouter_engine::files::write_private(&nullrouter_engine::tokens::path(dir.path()), &tokens).unwrap();
    let (engine, report) = Engine::open_parity(OperatorHome::new(dir.path())).unwrap();
    assert!(report.registry.unsupported.is_empty() && report.registry.skipped.is_empty(), "{report:#?}");
    Setup { _dir: dir, engine: Arc::new(engine), mock }
}

/// A whole Responses answer saying "hi", streamed (a `force_stream` endpoint).
pub fn responses_stream() -> Step {
    let usage = json!({"input_tokens": 3, "output_tokens": 1, "total_tokens": 4});
    let item = json!({"type": "message", "id": "msg_up", "status": "completed", "role": "assistant", "content": [{"type": "output_text", "text": "hi", "annotations": []}]});
    let done = json!({"id": "resp_up", "object": "response", "created_at": 1, "status": "completed", "model": "m1", "output": [item], "usage": usage});
    let started = json!({"id": "resp_up", "object": "response", "created_at": 1, "status": "in_progress", "model": "m1", "output": []});
    let ev = |name: &'static str, v: Value| (Some(name), v);
    Step::sse(
        &[
            ev("response.created", json!({"type": "response.created", "sequence_number": 0, "response": started})),
            ev(
                "response.output_item.added",
                json!({"type": "response.output_item.added", "sequence_number": 1, "output_index": 0, "item": {"type": "message", "id": "msg_up", "status": "in_progress", "role": "assistant", "content": []}}),
            ),
            ev(
                "response.output_text.delta",
                json!({"type": "response.output_text.delta", "sequence_number": 2, "item_id": "msg_up", "output_index": 0, "content_index": 0, "delta": "hi"}),
            ),
            ev(
                "response.output_item.done",
                json!({"type": "response.output_item.done", "sequence_number": 3, "output_index": 0, "item": item}),
            ),
            ev("response.completed", json!({"type": "response.completed", "sequence_number": 4, "response": done})),
        ],
        false,
    )
}

/// A unified model `u` over `members` (`(provider, model)`).
pub fn unified(members: &[(&str, &str)]) -> String {
    let m: Vec<String> = members.iter().map(|(p, m)| format!("{{ provider = \"{p}\", model = \"{m}\" }}")).collect();
    format!("[[unified_model]]\nname = \"u\"\nmembers = [{}]\n", m.join(", "))
}

pub fn chat_body(target: &str, stream: bool) -> Value {
    json!({"model": target, "stream": stream, "messages": [{"role": "user", "content": "hi"}]})
}

pub fn request(
    s: &Setup,
    client: &str,
    target: &str,
    body: Value,
    agent: &str,
    cancel: CancellationToken,
) -> TextRequest {
    let st = s.engine.snapshot();
    let client = st.style(client).unwrap().clone();
    let ir = request::decode(&client, &body).unwrap();
    let id = nullrouter_engine::records::new_id();
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
        count: false,
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
        (
            None,
            json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}),
        )
    };
    Step::sse(
        &[
            chunk(json!({"role": "assistant", "content": "Hel"}), Value::Null),
            chunk(json!({"content": "lo"}), Value::Null),
            chunk(json!({}), json!("stop")),
            (
                None,
                json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [], "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}}),
            ),
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
            let auth = r
                .headers
                .get("authorization")
                .or_else(|| r.headers.get("x-api-key"))
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            auth.rsplit(&format!("{SECRET}-")[..]).next().unwrap_or("").to_owned()
        })
        .collect()
}
