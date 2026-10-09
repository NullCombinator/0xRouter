//! Spec 010 SC-006: the harness tag is display only. The same request through the same key
//! before and after `keys tag` is placed the same, reaches the provider as the same body, and gets
//! the same answer, and the secret issued before the tag still authenticates after it.

mod common;

use common::{chat_whole, server};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

async fn send(base: &str, key: &str) -> (u16, Value, String) {
    let r = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(key)
        .body(json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}]}).to_string())
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let id = r.headers().get(REQUEST_ID).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    (status, serde_json::from_slice(&r.bytes().await.unwrap()).unwrap_or(Value::Null), id)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tagged_key_is_served_exactly_as_it_was_untagged() {
    let s = server().await;
    s.mock.push([chat_whole(), chat_whole()]);

    let (status_a, body_a, id_a) = send(&s.base, &s.key).await;
    assert_eq!(status_a, 200);

    // Tag the key on disk, as `keys tag` does, and reload.
    let file = s.home().join(keys::FILE);
    let mut list = Keys::load(&file).unwrap();
    list.set_harness("laptop", Some("claude-code")).unwrap();
    list.save().unwrap();
    s.engine.reload().await.unwrap();
    assert!(Keys::load(&file).unwrap().iter().any(|k| k.harness.as_deref() == Some("claude-code")));

    let (status_b, body_b, id_b) = send(&s.base, &s.key).await;
    assert_eq!((status_a, &body_a), (status_b, &body_b), "the same answer");

    let sent = s.mock.received();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].json(), sent[1].json(), "the same upstream body");
    assert_eq!(sent[0].headers["authorization"], sent[1].headers["authorization"]);

    let (a, b) = (s.engine.records.get(&id_a).unwrap(), s.engine.records.get(&id_b).unwrap());
    assert_eq!(a.outcome, b.outcome);
    assert_eq!(a.served_by, b.served_by, "the same placement");
    assert_eq!(a.agent, b.agent, "the same agent");
}
