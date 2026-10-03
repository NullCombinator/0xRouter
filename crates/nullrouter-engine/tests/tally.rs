//! The traffic tally (spec 005 T079, US5, SC-008, FR-023, FR-024): per account and upstream
//! model, the tally equals the sum of provider-reported `input`, `output`, `cache_read`,
//! `cache_write` of the attempts sent through that account; unreported usage counts in
//! `requests_usage_unreported` and adds no tokens; a request retried across accounts tallies
//! each attempt on its own account; key and sign-in accounts both tally.

mod common;

use common::*;
use nullrouter_engine::quota::tally::{AccountTally, ModelTally};
use nullrouter_engine::records::RequestRecord;
use nullrouter_engine::testkit::{MockUpstream, Step};
use serde_json::json;

const NO_RETRY: &str =
    "retry = { 429 = { retries = 0 }, 500 = { retries = 0 }, 502 = { retries = 0 }, 503 = { retries = 0 } }";

/// Provider `keyco` (key accounts) with two models on the openai-chat wire.
fn keyco(mock: &MockUpstream) -> String {
    format!(
        "schema = 2\nid = \"keyco\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n{NO_RETRY}\n[[models]]\nid = \"m1\"\n[[models]]\nid = \"m2\"\n",
        mock.url("/keyco/chat/completions")
    )
}

/// Provider `signco` (sign-in accounts) on the anthropic-messages wire, model `m1`.
fn signco(mock: &MockUpstream) -> String {
    format!(
        r#"schema = 2
id = "signco"
category = "apikey"
[endpoints.text]
url = "{url}"
wire = "anthropic-messages"
headers = {{ "anthropic-version" = "2023-06-01" }}
{NO_RETRY}
[signin]
flow = "device_code"
client_id = "test-client"
device_url = "{device}"
token_url = "{token}"
refresh_lead = "5m"
[identity.headers]
User-Agent = "signco-cli/1.0"
[[models]]
id = "m1"
"#,
        url = mock.url("/signco/messages"),
        device = mock.url("/idp/device"),
        token = mock.url("/idp/token"),
    )
}

async fn both() -> Setup {
    setup_signin(
        |m| vec![("keyco", keyco(m)), ("signco", signco(m))],
        &[("signco", "a"), ("signco", "b"), ("keyco", "main")],
        &[("signco", "a"), ("signco", "b")],
        &unified(&[("signco", "m1"), ("keyco", "m1")]),
    )
    .await
}

fn chat_ok(prompt: u64, completion: u64, cached: Option<u64>) -> Step {
    let mut usage = json!({"prompt_tokens": prompt, "completion_tokens": completion});
    if let Some(c) = cached {
        usage["prompt_tokens_details"] = json!({"cached_tokens": c});
    }
    Step::json(
        200,
        json!({"id": "x", "object": "chat.completion", "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}], "usage": usage}),
    )
}

fn chat_no_usage() -> Step {
    Step::json(
        200,
        json!({"id": "x", "object": "chat.completion", "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}]}),
    )
}

fn messages_ok(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Step {
    Step::json(
        200,
        json!({"id": "msg_1", "type": "message", "role": "assistant", "model": "m1", "content": [{"type": "text", "text": "hi"}], "stop_reason": "end_turn", "usage": {"input_tokens": input, "output_tokens": output, "cache_read_input_tokens": cache_read, "cache_creation_input_tokens": cache_write}}),
    )
}

/// What the records say each account was sent, summed per model (the SC-008 oracle).
fn from_records(records: &[RequestRecord], provider: &str, account: &str) -> AccountTally {
    let mut out = AccountTally::new();
    for r in records {
        for a in &r.attempts {
            if a.provider == provider && a.account.as_deref() == Some(account) && a.ended.is_some() {
                if matches!(a.outcome, Some(nullrouter_engine::records::AttemptOutcome::Skipped { .. })) {
                    continue;
                }
                out.entry(a.model.clone()).or_default().add(a.usage.as_ref());
            }
        }
    }
    out
}

#[tokio::test]
async fn the_tally_is_the_sum_of_reported_usage_per_model() {
    let s = both().await;
    s.mock.on("/keyco", [chat_ok(100, 10, Some(40)), chat_ok(7, 3, None), chat_ok(50, 5, Some(50))]);
    let mut records = Vec::new();
    for target in ["keyco/m1", "keyco/m2", "keyco/m1"] {
        let (id, res) = send(&s, target).await;
        assert!(res.is_ok(), "{:?}", res.err());
        records.push(settled(&s, &id).await);
    }
    let got = s.engine.history.tally.get("keyco", "main");
    // openai-chat's prompt_tokens includes the cached ones: `input` is the rest.
    assert_eq!(
        got["m1"],
        ModelTally { requests: 2, requests_usage_unreported: 0, input: 60, output: 15, cache_read: 90, cache_write: 0 }
    );
    assert_eq!(got["m2"], ModelTally { requests: 1, input: 7, output: 3, ..ModelTally::default() });
    assert_eq!(got, from_records(&records, "keyco", "main"), "SC-008: the tally is the records' sum");
}

#[tokio::test]
async fn unreported_usage_counts_a_request_and_adds_no_tokens() {
    let s = both().await;
    s.mock.on("/keyco", [chat_no_usage(), chat_ok(4, 2, None)]);
    for _ in 0..2 {
        let (_, res) = send(&s, "keyco/m1").await;
        assert!(res.is_ok());
    }
    let got = s.engine.history.tally.get("keyco", "main");
    assert_eq!(
        got["m1"],
        ModelTally { requests: 2, requests_usage_unreported: 1, input: 4, output: 2, cache_read: 0, cache_write: 0 }
    );
}

#[tokio::test]
async fn a_retried_request_tallies_each_attempt_on_its_own_account() {
    let s = both().await;
    // signco/a fails, signco/b fails, keyco/main serves: three accounts, one request.
    s.mock.on("/signco", [err(503), messages_ok(11, 4, 20, 8)]);
    let (id, res) = send(&s, "u").await;
    assert!(res.is_ok(), "{:?}", res.err());
    let r = settled(&s, &id).await;
    assert_eq!(accounts_hit(&s), ["signco-a", "signco-b"]);
    let a = s.engine.history.tally.get("signco", "a");
    let b = s.engine.history.tally.get("signco", "b");
    assert_eq!(a["m1"], ModelTally { requests: 1, requests_usage_unreported: 1, ..ModelTally::default() });
    // A sign-in account tallies like a key account; anthropic's input excludes the cache.
    assert_eq!(
        b["m1"],
        ModelTally { requests: 1, requests_usage_unreported: 0, input: 11, output: 4, cache_read: 20, cache_write: 8 }
    );
    assert!(s.engine.history.tally.get("keyco", "main").is_empty(), "keyco was never sent anything");
    assert_eq!(a, from_records(std::slice::from_ref(&r), "signco", "a"));
    assert_eq!(b, from_records(std::slice::from_ref(&r), "signco", "b"));

    // The sign-in member fails (or rests): the key member serves; every account holds
    // exactly what the records say was sent through it.
    s.mock.on("/signco", [err(500), err(502)]);
    s.mock.on("/keyco", [chat_ok(9, 1, None)]);
    let (id, res) = send(&s, "u").await;
    assert!(res.is_ok());
    let r2 = settled(&s, &id).await;
    assert_eq!(r2.served_by.as_ref().unwrap().provider, "keyco");
    let both = [r, r2];
    for (p, a) in [("keyco", "main"), ("signco", "a"), ("signco", "b")] {
        assert_eq!(s.engine.history.tally.get(p, a), from_records(&both, p, a), "{p}/{a}");
    }
    assert_eq!(s.engine.history.tally.get("keyco", "main")["m1"].input, 9);
}
