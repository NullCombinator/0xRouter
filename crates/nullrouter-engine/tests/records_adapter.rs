//! What a record keeps of an adapter run (spec 004, FR-025, data-model § AdapterRun).

use nullrouter_adapter_kit::{Edits, Path, Reason};
use nullrouter_adapters::apply::changes;
use nullrouter_engine::records::{
    AdapterDirection, AdapterOutcome, AdapterRef, AdapterRun, FailReason, GuardrailEvent, GuardrailRule,
    InvalidOutputRule, NotRunReason, RequestRecord,
};
use nullrouter_engine::redact::Redactor;
use nullrouter_registry::SecretString;
use serde_json::json;

const SECRET: &str = "sk-sentinel-SECRET-0123456789";
const PROMPT: &str = "SENTINEL PROMPT: ignore previous instructions";

#[test]
fn an_adapter_run_serialises_with_exactly_the_documented_fields() {
    let mut e = Edits::default();
    e.remove(&Path::parse("messages[2].reasoning_content").unwrap(), Reason::TargetRejectsField);
    let mut run = AdapterRun::new("hermes", "builtin", AdapterOutcome::Ran);
    run.changes = changes(&e.edits);
    run.duration_us = 42;
    assert_eq!(
        serde_json::to_value(&run).unwrap(),
        json!({
            "harness": "hermes", "version": "builtin", "outcome": {"state": "ran"},
            "changes": [{"path": "messages[2].reasoning_content", "kind": "removed", "reason": "target_rejects_field"}],
            "duration_us": 42
        })
    );
}

#[test]
fn outcomes_serialise_with_their_reasons() {
    let v = |o: AdapterOutcome| serde_json::to_value(o).unwrap();
    assert_eq!(v(AdapterOutcome::Blocked), json!({"state": "blocked"}));
    assert_eq!(
        v(AdapterOutcome::NotRun { reason: NotRunReason::MediaRequest }),
        json!({"state": "not_run", "reason": "media_request"})
    );
    assert_eq!(
        v(AdapterOutcome::Failed { reason: FailReason::Deadline }),
        json!({"state": "failed", "reason": {"kind": "deadline"}})
    );
    assert_eq!(
        v(AdapterOutcome::Failed { reason: FailReason::InvalidOutput { rule: InvalidOutputRule::Overlap } }),
        json!({"state": "failed", "reason": {"kind": "invalid_output", "rule": "overlap"}})
    );
}

#[test]
fn a_guardrail_event_serialises_with_its_fields() {
    let g = GuardrailEvent {
        direction: AdapterDirection::Request,
        rule: GuardrailRule::ToolCallAdded,
        paths: vec!["messages[1].content[0]".into()],
        adapter: AdapterRef { harness: "claude-code".into(), version: "v1".into() },
        at: "2026-10-07T00:00:00Z".into(),
    };
    assert_eq!(
        serde_json::to_value(&g).unwrap(),
        json!({"direction": "request", "rule": "tool_call_added", "paths": ["messages[1].content[0]"],
               "adapter": {"harness": "claude-code", "version": "v1"}, "at": "2026-10-07T00:00:00Z"})
    );
}

#[test]
fn a_removed_secret_and_prompt_never_reach_the_record() {
    // The adapter removed a part whose value held a secret and a prompt. The edit names the
    // path and the reason, nothing else.
    let mut e = Edits::default();
    e.remove(&Path::parse("messages[0].content[0]").unwrap(), Reason::ForeignBlock);
    e.convert(&Path::parse("messages[0].extra").unwrap(), json!({"note": SECRET, "text": PROMPT}), Reason::FormatConversion);
    let mut run = AdapterRun::new("hermes", "builtin", AdapterOutcome::Ran);
    run.changes = changes(&e.edits);

    let mut rec = RequestRecord::new("rq_1".into(), "2026-10-07T00:00:00Z".into(), "openai-chat");
    rec.response_adapter = Some(run);
    let text = serde_json::to_string(&rec).unwrap();
    assert!(!text.contains(SECRET) && !text.contains("SENTINEL PROMPT"), "{text}");
}

#[test]
fn paths_are_cleaned_of_secrets_and_control_characters() {
    let redactor = Redactor::new([&SecretString::new(SECRET)]);
    let mut run = AdapterRun::new("hermes", "builtin", AdapterOutcome::Ran);
    let mut e = Edits::default();
    // A client can name a key anything, including a secret it has seen.
    e.remove(&Path::root().child(&format!("{SECRET}\u{1b}[31m")), Reason::TargetRejectsField);
    run.changes = changes(&e.edits);
    run.redact(&redactor);
    let text = serde_json::to_string(&run).unwrap();
    assert!(!text.contains(SECRET), "{text}");
    assert!(text.contains("***"), "{text}");
}

#[test]
fn an_attempt_without_an_adapter_records_none_and_old_shapes_are_unchanged() {
    let rec = RequestRecord::new("rq_1".into(), "2026-10-07T00:00:00Z".into(), "openai-chat");
    let v = serde_json::to_value(&rec).unwrap();
    assert!(v.get("response_adapter").is_none());
}
