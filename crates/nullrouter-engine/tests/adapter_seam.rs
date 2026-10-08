//! The adapter seam in the attempt loop (spec 004, T014), with a test adapter that removes every
//! part its selector matches. (e) and (f), the response side and a client disconnect during an
//! adapter call, belong with the response seam; (h) and (i) are still to be written.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use nullrouter_adapter_kit::{Context, Edits, Reason};
use nullrouter_adapters::runner::{AdapterRunner, Fixture};
use nullrouter_adapters::selector::{Selector, extract};
use nullrouter_engine::keys::Keys;
use nullrouter_engine::records::{AdapterOutcome, RequestRecord};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

type Seen = Arc<Mutex<Vec<Context>>>;

/// A test adapter named `name` that removes every part `selector` matches.
fn remover(name: &str, selector: &str, seen: &Seen) -> AdapterRunner {
    let selectors = vec![Selector::parse(selector).unwrap()];
    let for_run = selectors.clone();
    let seen = seen.clone();
    AdapterRunner::Fixture(Fixture {
        name: name.into(),
        selectors,
        request: Arc::new(move |ctx: &Context, body: &Value| {
            seen.lock().unwrap().push(ctx.clone());
            let mut out = Edits::default();
            for part in extract(body, &for_run) {
                out.remove(&part.path, Reason::TargetRejectsField);
            }
            out
        }),
    })
}

/// Issues a key bound to `harness` (or none) and returns its id. The engine reloads to see it.
fn key(s: &Setup, harness: Option<&str>) -> String {
    let path = s._dir.path().join("keys.toml");
    let mut keys = Keys::load(&path).unwrap();
    let id = keys.issue(&format!("k{}", keys.iter().count()), None).unwrap().1.id.clone();
    if let Some(h) = harness {
        keys.set_harness(&id, Some(nullrouter_engine::keys::HarnessName::new(h).unwrap())).unwrap();
    }
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();
    id
}

async fn alpha() -> Setup {
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "a1")], "").await;
    s.mock.push([ok()]);
    s
}

fn body_with_reasoning() -> Value {
    json!({"model": "alpha/m1", "stream": false, "messages": [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "content": "hello", "reasoning_content": "SENTINEL-REASONING"}
    ]})
}

async fn send_body(s: &Setup, client: &str, body: Value, agent: &str) -> RequestRecord {
    let req = request(s, client, "alpha/m1", body, agent, CancellationToken::new());
    let id = req.id.clone();
    s.engine.text(s.engine.snapshot(), req).await.unwrap();
    settled(s, &id).await
}

#[tokio::test]
async fn a_removed_field_never_reaches_the_upstream_and_the_attempt_lists_it() {
    let s = alpha().await;
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].reasoning_content", &seen));
    let id = key(&s, Some("fixture"));
    let rec = send_body(&s, "openai-chat", body_with_reasoning(), &id).await;

    let got = s.mock.received();
    assert_eq!(got.len(), 1);
    let sent = String::from_utf8_lossy(&got[0].body).to_string();
    assert!(!sent.contains("SENTINEL-REASONING") && !sent.contains("reasoning_content"), "{sent}");
    assert!(sent.contains("hello"), "the rest of the message goes on: {sent}");

    let run = rec.attempts[0].adapter.as_ref().expect("the attempt records the run");
    assert_eq!(run.outcome, AdapterOutcome::Ran);
    assert_eq!(run.changes.len(), 1);
    assert_eq!(run.changes[0].path, "messages[1].reasoning_content");
    let record = serde_json::to_string(&rec).unwrap();
    assert!(!record.contains("SENTINEL-REASONING"), "a record holds paths, never content");
}

#[tokio::test]
async fn a_key_without_a_harness_runs_no_adapter() {
    let s = alpha().await;
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].reasoning_content", &seen));
    let id = key(&s, None);
    let rec = send_body(&s, "openai-chat", body_with_reasoning(), &id).await;

    assert!(seen.lock().unwrap().is_empty());
    assert!(rec.attempts[0].adapter.is_none());
    assert!(String::from_utf8_lossy(&s.mock.received()[0].body).contains("SENTINEL-REASONING"));
}

#[tokio::test]
async fn a_harness_with_nothing_installed_runs_as_a_plain_client() {
    let s = alpha().await;
    let id = key(&s, Some("never-installed"));
    let rec = send_body(&s, "openai-chat", body_with_reasoning(), &id).await;

    assert!(String::from_utf8_lossy(&s.mock.received()[0].body).contains("SENTINEL-REASONING"));
    let run = rec.attempts[0].adapter.as_ref().unwrap();
    assert!(matches!(run.outcome, AdapterOutcome::NotRun { .. }), "{:?}", run.outcome);
}

#[tokio::test]
async fn the_context_carries_no_header_key_agent_or_session() {
    let s = alpha().await;
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].reasoning_content", &seen));
    let id = key(&s, Some("fixture"));
    send_body(&s, "openai-chat", body_with_reasoning(), &id).await;

    let ctxs = seen.lock().unwrap().clone();
    assert_eq!(ctxs.len(), 1);
    let text = serde_json::to_string(&ctxs[0]).unwrap();
    for secret in [id.as_str(), "sess-1", SECRET, "authorization", "Bearer"] {
        assert!(!text.contains(secret), "the context holds {secret:?}: {text}");
    }
    assert_eq!(ctxs[0].provider, "alpha");
    assert_eq!(ctxs[0].target_style, "openai-chat");
    assert!(ctxs[0].same_style);
    assert_eq!(ctxs[0].attempt, 1);
}

#[tokio::test]
async fn a_fallback_runs_the_adapter_again_with_its_own_context_and_changes() {
    let s = setup(
        |m| vec![("alpha", chat_plugin(m, "alpha", "")), ("beta", chat_plugin(m, "beta", ""))],
        &[("alpha", "a1"), ("beta", "b1")],
        &unified(&[("alpha", "m1"), ("beta", "m1")]),
    )
    .await;
    s.mock.on("/alpha", [err(503)]);
    s.mock.on("/beta", [ok()]);
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].reasoning_content", &seen));
    let id = key(&s, Some("fixture"));
    let req = request(&s, "openai-chat", "u", body_with_reasoning(), &id, CancellationToken::new());
    let rid = req.id.clone();
    s.engine.text(s.engine.snapshot(), req).await.unwrap();
    let rec = settled(&s, &rid).await;

    let ctxs = seen.lock().unwrap().clone();
    let providers: Vec<&str> = ctxs.iter().map(|c| c.provider.as_str()).collect();
    assert!(providers.contains(&"alpha") && providers.contains(&"beta"), "{providers:?}");
    assert!(ctxs.windows(2).all(|w| w[0].attempt < w[1].attempt), "attempt numbers rise: {ctxs:?}");
    for a in rec.attempts.iter().filter(|a| a.adapter.is_some()) {
        assert_eq!(a.adapter.as_ref().unwrap().changes.len(), 1, "each attempt has its own changes");
    }
    assert!(rec.attempts.iter().filter(|a| a.adapter.is_some()).count() >= 2);
}

#[tokio::test]
async fn a_cross_style_attempt_encodes_from_the_edited_request() {
    let s = alpha().await;
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].content[1]", &seen));
    let id = key(&s, Some("fixture"));
    // Messages in, Chat Completions out. The adapter removes the second content block; the
    // upstream body is encoded from the request decoded again from the edited body, so it
    // lacks that block too.
    let body = json!({"model": "alpha/m1", "max_tokens": 16, "messages": [{"role": "user", "content": [
        {"type": "text", "text": "KEEP-ME"},
        {"type": "text", "text": "DROP-ME"}
    ]}]});
    let rec = send_body(&s, "anthropic-messages", body, &id).await;

    let ctxs = seen.lock().unwrap().clone();
    assert_eq!(ctxs.len(), 1);
    assert!(!ctxs[0].same_style);
    assert_eq!(ctxs[0].target_style, "openai-chat");
    let sent = String::from_utf8_lossy(&s.mock.received()[0].body).to_string();
    assert!(sent.contains("KEEP-ME") && !sent.contains("DROP-ME"), "{sent}");
    let run = rec.attempts[0].adapter.as_ref().unwrap();
    assert_eq!(run.outcome, AdapterOutcome::Ran);
    assert_eq!(run.changes[0].path, "messages[0].content[1]");
}
