use std::borrow::Cow;

use nullrouter_adapter_kit::{Capabilities, Context, Direction};
use nullrouter_adapters::HarnessName;
use nullrouter_adapters::record::{AdapterOutcome, NotRunReason};
use nullrouter_adapters::runner::AdapterRunner;
use serde_json::json;

fn ctx() -> Context {
    Context {
        direction: Direction::Request,
        provider: "groq".into(),
        target_style: "openai-chat".into(),
        same_style: true,
        model: "m".into(),
        model_type: "text".into(),
        capabilities: Capabilities::default(),
        stream: false,
        attempt: 1,
    }
}

#[test]
fn builtins_resolve_by_name_and_unknown_names_do_not() {
    assert!(AdapterRunner::builtin(&HarnessName::new("hermes").unwrap()).is_some());
    assert!(AdapterRunner::builtin(&HarnessName::new("claude-code").unwrap()).is_none());
}

#[tokio::test]
async fn a_run_with_no_edits_borrows_the_body() {
    let r = AdapterRunner::builtin(&HarnessName::new("hermes").unwrap()).unwrap();
    let body = json!({"messages": [{"role": "user", "content": "hi"}]});
    let out = r.run_request(&ctx(), &body).await;
    assert!(matches!(out.body, Cow::Borrowed(_)));
    assert_eq!(out.run.outcome, AdapterOutcome::Ran);
    assert_eq!((out.run.harness.as_str(), out.run.version.as_str()), ("hermes", "builtin"));
    assert!(out.run.changes.is_empty());
}

#[tokio::test]
async fn the_response_side_reports_no_selector_match_for_hermes() {
    let r = AdapterRunner::builtin(&HarnessName::new("hermes").unwrap()).unwrap();
    let body = json!({});
    let out = r.run_response(&ctx(), &body).await;
    assert_eq!(out.run.outcome, AdapterOutcome::NotRun { reason: NotRunReason::NoSelectorMatch });
}
