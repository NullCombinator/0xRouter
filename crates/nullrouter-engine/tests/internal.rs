//! Internal requests through the engine (spec 004, T058).

mod common;

use common::*;
use nullrouter_engine::internal::InternalRequest;
use nullrouter_engine::records::Outcome;
use serde_json::Value;

#[tokio::test]
async fn an_internal_request_goes_upstream_and_is_recorded_under_its_label() {
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "a1")], "").await;
    s.mock.push([ok()]);
    let label = "review:acme@v1.0.0-xx";
    let answer = s
        .engine
        .internal(InternalRequest {
            agent_label: label.into(),
            model: "alpha/m1".into(),
            system: "SYSTEM-TEXT".into(),
            user: "USER-TEXT".into(),
            max_tokens: 77,
        })
        .await
        .unwrap();

    let sent: Value = serde_json::from_slice(&s.mock.received()[0].body).unwrap();
    assert_eq!(sent["max_tokens"], 77, "{sent}");
    let text = sent.to_string();
    assert!(text.contains("SYSTEM-TEXT") && text.contains("USER-TEXT"), "{text}");

    assert_eq!(answer.text, "hi");
    assert_eq!(answer.tokens_in, Some(3));
    assert_eq!(answer.tokens_out, Some(1));

    let rec = settled(&s, &answer.record_id).await;
    assert_eq!(rec.outcome, Outcome::Succeeded);
    assert_eq!(rec.agent.as_ref().unwrap().key, label);
    assert!(rec.attempts.iter().all(|a| a.adapter.is_none()), "{:?}", rec.attempts);
    assert!(rec.response_adapter.is_none());
}
