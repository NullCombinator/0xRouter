//! A running server over a scripted OpenAI-compatible provider `mockco` (model `m1`) with
//! one agent key, shared by the text and harness tests.

// Each test binary uses only part of this module.
#![allow(dead_code)]

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::net::TcpListener;
use zerorouter_engine::keys::{self, Keys};
use zerorouter_engine::state::Engine;
use zerorouter_engine::testkit::{MockUpstream, Step};
use zerorouter_registry::OperatorHome;
use zerorouter_server::serve::{App, run};

pub const SECRET: &str = "sk-mock-SENTINEL-0002";

pub struct Server {
    _dir: tempfile::TempDir,
    pub engine: Arc<Engine>,
    pub mock: MockUpstream,
    pub base: String,
    pub key: String,
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
    let mock = MockUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "allow_private_endpoints = true\n").unwrap();
    std::fs::create_dir(dir.path().join("plugins")).unwrap();
    let plugin = format!(
        "schema = 2\nid = \"mockco\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
        mock.url("/v1/chat/completions")
    );
    std::fs::write(dir.path().join("plugins/mockco.toml"), plugin).unwrap();
    let accounts = format!("schema = 1\n[[account]]\nprovider = \"mockco\"\nname = \"main\"\nsecret = \"{SECRET}\"\n");
    zerorouter_engine::files::write_private(&dir.path().join(zerorouter_engine::accounts::FILE), &accounts).unwrap();
    let mut keys = Keys::default();
    let (key, _) = keys.issue("laptop", None).unwrap();
    zerorouter_engine::files::write_private(&dir.path().join(keys::FILE), &keys.to_toml()).unwrap();

    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    let engine = Arc::new(engine);
    let app = App::new(engine.clone()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(run(app, listener, async {
        let _ = stopped.await;
    }));
    Server { _dir: dir, engine, mock, base, key, stop: Some(stop) }
}

/// A whole chat completion saying "hi there".
pub fn chat_whole() -> Step {
    chat_whole_text("hi there")
}

pub fn chat_whole_text(text: &str) -> Step {
    Step::json(
        200,
        json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5}}),
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
