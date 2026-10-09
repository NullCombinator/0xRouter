//! The adapter review (spec 004, T051), against a mock provider.
//!
//! These live in the engine's tests, not the adapters', because the engine depends on the
//! adapters crate: a dev-dependency back would give two copies of its types.

mod common;

use common::*;
use nullrouter_adapters::HarnessName;
use nullrouter_adapters::review::SYSTEM_PROMPT;
use nullrouter_adapters::store::{ReviewConfig, Store, VersionId, VersionState};
use nullrouter_adapters::testkit::{Behaviour, install_fixture, install_in_review, wat_adapter};
use nullrouter_engine::testkit::Step;
use serde_json::{Value, json};

const MANIFEST: &str = r#"
harness = "acme"
style = "openai-chat"
kit = "1"

[request]
selectors = ["messages[*].reasoning_content"]
"#;
const LIB: &str = "pub fn secret_helper_name(body_field: u32) -> u32 { body_field + 1 }\n";
const GOOD: &str = r#"{"risk":"low","summary":"adds one","findings":[{"location":"src/lib.rs:1","concern":"none"}]}"#;

fn reply(text: &str, tokens_in: u64, tokens_out: u64) -> Step {
    Step::json(
        200,
        json!({"id": "x", "object": "chat.completion", "model": "m1",
               "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
               "usage": {"prompt_tokens": tokens_in, "completion_tokens": tokens_out}}),
    )
}

async fn rig() -> Setup {
    setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "a1")], "").await
}

fn harness() -> HarnessName {
    HarnessName::new("acme").unwrap()
}

fn set_review(s: &Setup, budget: u64, reserve: u32) {
    let store = Store::open(s._dir.path()).unwrap();
    let mut index = store.load_index().unwrap();
    index.review = Some(ReviewConfig { model: "alpha/m1".into(), budget_tokens: budget, reserve_output: reserve });
    store.save_index(&index).unwrap();
}

/// An approved 1.0.0 that serves, and a new 1.1.0 in review.
fn install(s: &Setup) -> (VersionId, VersionId) {
    let wasm = wat_adapter(Behaviour::NoEdits);
    let old = install_fixture(s._dir.path(), "acme", MANIFEST, &wasm);
    let new = install_in_review(s._dir.path(), "acme", MANIFEST, &wasm, (1, 1, 0), LIB);
    (old, new)
}

fn entry(s: &Setup, id: &VersionId) -> (VersionState, String) {
    let index = Store::open(s._dir.path()).unwrap().load_index().unwrap();
    let v = index.version(&harness(), id).unwrap();
    (v.state, v.state_reason.clone())
}

fn serving(s: &Setup) -> Option<VersionId> {
    let index = Store::open(s._dir.path()).unwrap().load_index().unwrap();
    index.serving(&harness()).map(|v| v.id.clone())
}

#[tokio::test]
async fn no_review_settings_quarantine_without_a_request() {
    let s = rig().await;
    let (_, new) = install(&s);
    let end = s.engine.review_now(&harness(), &new).await.unwrap();
    assert_eq!(end.state, VersionState::Quarantined);
    let (state, reason) = entry(&s, &new);
    assert_eq!(state, VersionState::Quarantined);
    assert!(reason.contains("no_review_model"), "{reason}");
    assert!(s.mock.received().is_empty());
}

#[tokio::test]
async fn a_budget_too_small_quarantines_with_both_numbers_and_no_request() {
    let s = rig().await;
    let (_, new) = install(&s);
    set_review(&s, 4200, 4096);
    s.engine.review_now(&harness(), &new).await.unwrap();
    let (state, reason) = entry(&s, &new);
    assert_eq!(state, VersionState::Quarantined);
    assert!(reason.contains("have 4200") && reason.contains("need 4"), "{reason}");
    assert!(s.mock.received().is_empty());
}

#[tokio::test]
async fn the_request_holds_the_prompt_and_scrambled_source_only() {
    let s = rig().await;
    let (_, new) = install(&s);
    set_review(&s, 60_000, 777);
    s.mock.push([reply(GOOD, 100, 20)]);
    s.engine.review_now(&harness(), &new).await.unwrap();
    let got = s.mock.received();
    assert_eq!(got.len(), 1);
    let sent: Value = serde_json::from_slice(&got[0].body).unwrap();
    assert!(sent.get("tools").is_none(), "{sent}");
    assert_eq!(sent["max_tokens"], 777);
    let messages = sent["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], SYSTEM_PROMPT);
    let user = messages[1]["content"].as_str().unwrap();
    assert!(user.contains("messages[*].reasoning_content"), "{user}");
    assert!(user.contains("src/lib.rs") && user.contains("v1"), "{user}");
    assert!(!user.contains("secret_helper_name") && !user.contains("body_field"), "{user}");
    let text = sent.to_string();
    assert!(!text.contains(SECRET) && !text.contains("line_map") && !text.contains("req_"), "{text}");
}

#[tokio::test]
async fn a_malformed_report_gets_one_retry_then_quarantine() {
    let s = rig().await;
    let (_, new) = install(&s);
    set_review(&s, 60_000, 4096);
    s.mock.push([ok(), ok()]);
    s.engine.review_now(&harness(), &new).await.unwrap();
    assert_eq!(s.mock.received().len(), 2);
    let (state, reason) = entry(&s, &new);
    assert_eq!(state, VersionState::Quarantined);
    assert!(reason.contains("review failed"), "{reason}");
}

#[tokio::test]
async fn a_malformed_report_then_a_good_one_is_reported() {
    let s = rig().await;
    let (_, new) = install(&s);
    set_review(&s, 60_000, 4096);
    s.mock.push([ok(), reply(GOOD, 100, 20)]);
    s.engine.review_now(&harness(), &new).await.unwrap();
    assert_eq!(entry(&s, &new).0, VersionState::Reported);
}

#[tokio::test]
async fn no_retry_when_the_tokens_used_leave_no_room() {
    let s = rig().await;
    let (_, new) = install(&s);
    set_review(&s, 20_000, 1000);
    s.mock.push([reply("not json", 19_000, 90)]);
    s.engine.review_now(&harness(), &new).await.unwrap();
    assert_eq!(s.mock.received().len(), 1);
    let (state, reason) = entry(&s, &new);
    assert_eq!(state, VersionState::Quarantined);
    assert!(reason.contains("budget exhausted") && reason.contains("have 20000"), "{reason}");
}

#[tokio::test]
async fn a_provider_error_quarantines_and_the_active_version_keeps_serving() {
    let s = rig().await;
    let (old, new) = install(&s);
    set_review(&s, 60_000, 4096);
    s.mock.push([err(400)]);
    s.engine.review_now(&harness(), &new).await.unwrap();
    let (state, reason) = entry(&s, &new);
    assert_eq!(state, VersionState::Quarantined);
    assert!(reason.contains("review failed"), "{reason}");
    assert!(!reason.contains(SECRET));
    assert_eq!(serving(&s), Some(old));
}

#[tokio::test]
async fn a_valid_report_is_written_and_the_request_recorded_under_the_review_label() {
    let s = rig().await;
    let (old, new) = install(&s);
    set_review(&s, 60_000, 4096);
    s.mock.push([reply(GOOD, 100, 20)]);
    let end = s.engine.review_now(&harness(), &new).await.unwrap();
    assert_eq!(end.state, VersionState::Reported);
    assert_eq!(entry(&s, &new).0, VersionState::Reported);
    assert_eq!(serving(&s), Some(old));

    let store = Store::open(s._dir.path()).unwrap();
    let path = store.version_dir(&harness(), &new).join("review.json");
    let report: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(report["risk"], "low");
    assert_eq!(report["tokens_in"], 100);
    assert_eq!(report["tokens_out"], 20);
    assert!(report["model"].is_string() && report.get("provider").is_some());
    assert!(report["files"].is_array());

    let id = report["record"].as_str().unwrap();
    let rec = settled(&s, id).await;
    assert_eq!(rec.agent.as_ref().unwrap().key, format!("review:acme@{new}"));
}
