//! A running server over a scripted OpenAI-compatible provider `mockco` (model `m1`) with
//! one agent key, shared by the text and harness tests.

// Each test binary uses only part of this module.
#![allow(dead_code)]

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::net::TcpListener;
use zerorouter_engine::keys::{self, Keys};
use zerorouter_engine::state::Engine;
use zerorouter_engine::testkit::{MockUpstream, Received, Step};
use zerorouter_registry::OperatorHome;
use zerorouter_server::serve::{App, run};

pub const SECRET: &str = "sk-mock-SENTINEL-0002";

pub struct Server {
    _dir: tempfile::TempDir,
    pub engine: Arc<Engine>,
    pub mock: MockUpstream,
    pub base: String,
    pub key: String,
    /// A key issued and then revoked.
    pub revoked: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
    }
}

pub async fn server() -> Server {
    server_with(|_| Vec::new()).await
}

/// The server with more providers: `(id, plugin TOML)` pairs, each given an account.
pub async fn server_with(extra: impl FnOnce(&MockUpstream) -> Vec<(&'static str, String)>) -> Server {
    let mock = MockUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "allow_private_endpoints = true\n").unwrap();
    std::fs::create_dir(dir.path().join("plugins")).unwrap();
    let mut plugins = extra(&mock);
    plugins.push((
        "mockco",
        format!(
            "schema = 2\nid = \"mockco\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
            mock.url("/v1/chat/completions")
        ),
    ));
    let mut accounts = String::from("schema = 1\n");
    for (id, toml) in &plugins {
        std::fs::write(dir.path().join(format!("plugins/{id}.toml")), toml).unwrap();
        accounts += &format!("[[account]]\nprovider = \"{id}\"\nname = \"main\"\nsecret = \"{SECRET}\"\n");
    }
    zerorouter_engine::files::write_private(&dir.path().join(zerorouter_engine::accounts::FILE), &accounts).unwrap();
    let mut keys = Keys::default();
    let (key, _) = keys.issue("laptop", None).unwrap();
    let (revoked, _) = keys.issue("old", None).unwrap();
    keys.revoke("old").unwrap();
    zerorouter_engine::files::write_private(&dir.path().join(keys::FILE), &keys.to_toml()).unwrap();

    let (engine, report) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    let engine = Arc::new(engine);
    let app = App::new(engine.clone()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(run(app, listener, async {
        let _ = stopped.await;
    }));
    Server { _dir: dir, engine, mock, base, key, revoked, stop: Some(stop) }
}

/// A whole chat completion saying "hi there".
pub fn chat_whole() -> Step {
    chat_whole_text("hi there")
}

pub fn chat_whole_text(text: &str) -> Step {
    Step::json(
        200,
        json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}}),
    )
}

/// A streamed chat completion saying "Hello" in two deltas, then usage.
pub fn chat_stream() -> Step {
    let chunk = |delta: Value, finish: Value| {
        (None, json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}))
    };
    Step::sse(
        &[
            chunk(json!({"role": "assistant", "content": "Hel"}), Value::Null),
            chunk(json!({"content": "lo"}), Value::Null),
            chunk(json!({}), json!("stop")),
            (None, json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1", "choices": [], "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}})),
        ],
        true,
    )
}

/// A provider `multi` with one endpoint per text wire under the mock, and one model per
/// wire: `m-chat`, `m-messages`, `m-responses`.
pub fn multi_plugin(mock: &MockUpstream) -> (&'static str, String) {
    let toml = format!(
        r#"schema = 2
id = "multi"
category = "apikey"
[auth]
kind = "apikey"
[[endpoints.text]]
url = "{chat}"
wire = "openai-chat"
[[endpoints.text]]
url = "{messages}"
wire = "anthropic-messages"
headers = {{ "anthropic-version" = "2023-06-01" }}
auth = {{ header = "x-api-key", scheme = "raw" }}
[[endpoints.text]]
url = "{responses}"
wire = "openai-responses"
[[models]]
id = "m-chat"
wires = ["openai-chat"]
[[models]]
id = "m-messages"
wires = ["anthropic-messages"]
[[models]]
id = "m-responses"
wires = ["openai-responses"]
"#,
        chat = mock.url("/multi/chat/completions"),
        messages = mock.url("/multi/messages"),
        responses = mock.url("/multi/responses"),
    );
    ("multi", toml)
}

/// `broken/m1`: every request gets a 401, so every attempt fails (under [`reply_by_wire`]).
pub fn broken_plugin(mock: &MockUpstream) -> (&'static str, String) {
    let toml = format!(
        "schema = 2\nid = \"broken\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
        mock.url("/broken/chat/completions")
    );
    ("broken", toml)
}

/// A reply saying "Hello" with 5 input and 2 output tokens, in the wire the request's path
/// names, streamed when the request asks for it.
pub fn reply_by_wire(r: &Received) -> Step {
    let stream = r.json()["stream"] == Value::Bool(true);
    let path = r.path_and_query.as_str();
    if path.starts_with("/broken") {
        return Step::json(401, json!({"error": {"message": "invalid api key"}}));
    }
    if path.contains("/messages") {
        if !stream {
            return Step::json(
                200,
                json!({"id": "msg_up", "type": "message", "role": "assistant", "model": "m-messages", "content": [{"type": "text", "text": "Hello"}], "stop_reason": "end_turn", "stop_sequence": null, "usage": {"input_tokens": 5, "output_tokens": 2}}),
            );
        }
        let ev = |name: &'static str, v: Value| (Some(name), v);
        return Step::sse(
            &[
                ev("message_start", json!({"type": "message_start", "message": {"id": "msg_up", "type": "message", "role": "assistant", "model": "m-messages", "content": [], "stop_reason": null, "usage": {"input_tokens": 5, "output_tokens": 1}}})),
                ev("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
                ev("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hel"}})),
                ev("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "lo"}})),
                ev("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
                ev("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 2}})),
                ev("message_stop", json!({"type": "message_stop"})),
            ],
            false,
        );
    }
    if path.contains("/responses") {
        let usage = json!({"input_tokens": 5, "output_tokens": 2, "total_tokens": 7});
        let item = json!({"type": "message", "id": "msg_up", "status": "completed", "role": "assistant", "content": [{"type": "output_text", "text": "Hello", "annotations": []}]});
        let done = json!({"id": "resp_up", "object": "response", "created_at": 1, "status": "completed", "model": "m-responses", "output": [item], "usage": usage});
        if !stream {
            return Step::json(200, done);
        }
        let started = json!({"id": "resp_up", "object": "response", "created_at": 1, "status": "in_progress", "model": "m-responses", "output": []});
        let ev = |name: &'static str, v: Value| (Some(name), v);
        return Step::sse(
            &[
                ev("response.created", json!({"type": "response.created", "sequence_number": 0, "response": started})),
                ev("response.output_item.added", json!({"type": "response.output_item.added", "sequence_number": 1, "output_index": 0, "item": {"type": "message", "id": "msg_up", "status": "in_progress", "role": "assistant", "content": []}})),
                ev("response.content_part.added", json!({"type": "response.content_part.added", "sequence_number": 2, "item_id": "msg_up", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}})),
                ev("response.output_text.delta", json!({"type": "response.output_text.delta", "sequence_number": 3, "item_id": "msg_up", "output_index": 0, "content_index": 0, "delta": "Hel"})),
                ev("response.output_text.delta", json!({"type": "response.output_text.delta", "sequence_number": 4, "item_id": "msg_up", "output_index": 0, "content_index": 0, "delta": "lo"})),
                ev("response.output_text.done", json!({"type": "response.output_text.done", "sequence_number": 5, "item_id": "msg_up", "output_index": 0, "content_index": 0, "text": "Hello"})),
                ev("response.content_part.done", json!({"type": "response.content_part.done", "sequence_number": 6, "item_id": "msg_up", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": "Hello", "annotations": []}})),
                ev("response.output_item.done", json!({"type": "response.output_item.done", "sequence_number": 7, "output_index": 0, "item": item})),
                ev("response.completed", json!({"type": "response.completed", "sequence_number": 8, "response": done})),
            ],
            false,
        );
    }
    if stream { chat_stream() } else { chat_whole_text("Hello") }
}
