//! Secrets stay bound to the hosts their provider used when the account was first seen
//! (T142): a hand-written account's binding is saved at load, and a replacing plugin that
//! sends token counts to another host gets no secret.

mod common;

use common::{SECRET, messages_plugin, request, setup};
use nullrouter_engine::accounts::FILE;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_new_token_count_host_gets_no_secret() {
    let s = setup(|m| vec![("mockco", messages_plugin(m, "mockco"))], &[("mockco", "main")], "").await;
    let home = s._dir.path();
    let saved = std::fs::read_to_string(home.join(FILE)).unwrap();
    assert!(saved.contains("127.0.0.1"), "the first binding is saved:\n{saved}");

    // The same endpoint, plus a count URL on a name the account was never bound to.
    let count = s.mock.url("/count").replace("127.0.0.1", "localhost");
    let plugin = format!("{}[endpoints.text.token_count]\nurl = \"{count}\"\n", messages_plugin(&s.mock, "mockco"));
    std::fs::write(home.join("plugins/mockco.toml"), plugin).unwrap();
    s.engine.reload().await.unwrap();

    let body = json!({"model": "mockco/m1", "max_tokens": 8, "messages": [{"role": "user", "content": "hi"}]});
    let mut req = request(&s, "anthropic-messages", "mockco/m1", body, "ak_test", CancellationToken::new());
    req.count = true;
    let Err(f) = s.engine.text(s.engine.snapshot(), req).await else { panic!("the secret was released") };
    assert!(f.tried.iter().any(|t| t.reason.contains("localhost")), "{:?}", f.tried);
    assert!(s.mock.received().is_empty(), "nothing reaches either host");
    assert!(!f.message.contains(SECRET));
}
