//! Slice 005's deliberate deviations from 9router (research R19, spec Clarifications Q1, Q6;
//! T039), asserted on the request that reaches the provider. The plugins are the bundled
//! anthropic and grok-cli files with their hosts pointed at the mock, so these tests break
//! if a bundled file starts asking for a body change.
//!
//! - No Claude cloaking: a signed-in anthropic request's body is the client's, byte for
//!   byte; no tool renaming, decoy tools, injected system text or invented ids.
//! - No grok-cli input rewrites, field allowlist or always-forced `store`,
//!   `reasoning.summary`, `include`: the input items reach the provider untouched, and only
//!   an effort-suffixed model id adds `reasoning.effort`, which the record notes.
//! - `x-grok-agent-id` is this installation's id, not a machine-id hash.

mod common;

use common::*;
use nullrouter_engine::records::RequestRecord;
use nullrouter_engine::testkit::{MockUpstream, Received, Step};
use reqwest::header::HeaderValue;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// `plugins/bundled/<id>.toml` with every `https://host` the core sends to pointed at the
/// mock. Redirect URIs stay: the browser, not the core, follows them.
fn bundled_at_mock(mock: &MockUpstream, id: &str) -> String {
    let path = format!("{}/../../plugins/bundled/{id}.toml", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let base = mock.url("");
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let mut rest = line;
        while let Some(i) = rest.find("https://").filter(|_| !line.contains("uri =")) {
            out.push_str(&rest[..i]);
            out.push_str(&base);
            let after = &rest[i + "https://".len()..];
            rest = &after[after.find(['/', '"']).unwrap_or(after.len())..];
        }
        out.push_str(rest);
        out.push('\n');
    }
    out
}

async fn signed_in() -> Setup {
    setup_signin(
        |m| vec![("anthropic", bundled_at_mock(m, "anthropic")), ("grok-cli", bundled_at_mock(m, "grok-cli"))],
        &[("anthropic", "max"), ("grok-cli", "work")],
        &[("anthropic", "max"), ("grok-cli", "work")],
        "[plugin_decisions]\nanthropic = \"replace\"\n\"grok-cli\" = \"replace\"\n",
    )
    .await
}

fn messages_answer() -> Step {
    Step::json(
        200,
        json!({"id": "msg_up", "type": "message", "role": "assistant", "model": "claude-sonnet-4-20250514", "content": [{"type": "text", "text": "hi"}], "stop_reason": "end_turn", "stop_sequence": null, "usage": {"input_tokens": 3, "output_tokens": 1}}),
    )
}

/// Sends `body` from a `client`-style client to `target`, with `headers`; returns the
/// record and what the mock received.
async fn send_body(
    s: &Setup,
    client: &str,
    target: &str,
    body: Value,
    headers: &[(&'static str, &'static str)],
) -> (RequestRecord, Received) {
    let mut req = request(s, client, target, body, "ak_test", CancellationToken::new());
    for (k, v) in headers {
        req.headers.insert(*k, HeaderValue::from_static(v));
    }
    let id = req.id.clone();
    let res = s.engine.text(s.engine.snapshot(), req).await;
    assert!(res.is_ok(), "{:?}", res.err());
    let r = settled(s, &id).await;
    (r, s.mock.received().pop().expect("the provider got the request"))
}

/// A Claude Code-shaped Messages body: tools, a system prompt, metadata.
fn claude_code_body() -> Value {
    json!({
        "model": "claude-sonnet-4-20250514",
        "max_tokens": 64,
        "system": [{"type": "text", "text": "You are a coding agent."}],
        "metadata": {"user_id": "client-user-1"},
        "tools": [{"name": "Read", "description": "read a file", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}}],
        "messages": [{"role": "user", "content": "hi"}]
    })
}

#[tokio::test]
async fn anthropic_sign_in_body_is_the_clients_byte_for_byte() {
    let s = signed_in().await;
    s.mock.on("/v1/messages", [messages_answer()]);
    let body = claude_code_body();
    let (r, got) = send_body(
        &s,
        "anthropic-messages",
        "anthropic/claude-sonnet-4-20250514",
        body.clone(),
        &[("anthropic-beta", "claude-code-20250219,interleaved-thinking-2025-05-14")],
    )
    .await;
    assert_eq!(got.body, serde_json::to_vec(&body).unwrap(), "no cloaking: the body is the client's");
    assert!(r.attempts[0].forced.is_empty());

    let h = &got.headers;
    assert_eq!(h["authorization"], format!("Bearer {SECRET}-anthropic-max"));
    assert!(h.get("x-api-key").is_none(), "the subscription token replaces the key header");
    assert_eq!(
        h["anthropic-beta"], "claude-code-20250219,interleaved-thinking-2025-05-14,oauth-2025-04-20",
        "the identity beta joins the client's list"
    );
    assert_eq!(h["anthropic-version"], "2023-06-01");
    assert_eq!(h["user-agent"], "claude-cli/2.1.280 (external, sdk-cli)");
    assert_eq!(h["x-app"], "cli");
}

#[tokio::test]
async fn anthropic_sign_in_adds_no_system_text_or_tools_to_a_translated_body() {
    let s = signed_in().await;
    s.mock.on("/v1/messages", [messages_answer()]);
    let body = json!({
        "model": "anthropic/claude-sonnet-4-20250514",
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [{"type": "function", "function": {"name": "read_file", "parameters": {"type": "object"}}}]
    });
    let (_, got) = send_body(&s, "openai-chat", "anthropic/claude-sonnet-4-20250514", body, &[]).await;
    let sent = got.json();
    assert!(sent.get("system").is_none(), "no injected system text: {sent}");
    assert!(sent.get("metadata").is_none(), "no invented user id: {sent}");
    let tools: Vec<&str> = sent["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(tools, ["read_file"], "no renamed or decoy tools");
    assert_eq!(got.headers["anthropic-beta"], "oauth-2025-04-20", "a cross-style client sent no beta list");
}

/// A Codex-shaped Responses body: a reasoning item with encrypted content, an orphan tool
/// output and another server's item ids, which 9router's grok-cli executor rewrites.
fn codex_body(model: &str) -> Value {
    json!({
        "model": model,
        "stream": true,
        "instructions": "You are Codex.",
        "store": true,
        "input": [
            {"type": "message", "id": "msg_openai_1", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
            {"type": "reasoning", "id": "rs_openai_2", "summary": [], "encrypted_content": "gAAAA-opaque"},
            {"type": "function_call_output", "call_id": "call_gone", "output": "orphan"},
            {"type": "item_reference", "id": "msg_openai_3"}
        ],
        "parallel_tool_calls": false,
        "prompt_cache_key": "client-cache-key"
    })
}

#[tokio::test]
async fn grok_cli_input_items_reach_the_provider_untouched() {
    let s = signed_in().await;
    s.mock.on("/v1/responses", [responses_stream()]);
    let body = codex_body("grok-4.5");
    let (r, got) = send_body(&s, "openai-responses", "grok-cli/grok-4.5", body.clone(), &[]).await;
    assert_eq!(got.body, serde_json::to_vec(&body).unwrap(), "no input rewrites, allowlist or forced fields");
    assert!(r.attempts[0].forced.is_empty());
    let install = s.engine.install_id().unwrap().to_owned();
    assert_eq!(got.headers["x-grok-agent-id"], install.as_str(), "the installation's own id");
    assert_eq!(got.headers["x-email"], "work@example.com");
    assert_eq!(got.headers["x-userid"], "user-work");
    assert_eq!(got.headers["x-grok-model-override"], "grok-4.5");
    assert_eq!(got.headers["x-grok-session-id"], "sess-1", "the agent's own session");
    assert_eq!(got.headers["x-grok-conv-id"], "sess-1");
    assert_eq!(got.headers["x-grok-turn-idx"], "1");
    assert_eq!(got.headers["authorization"], format!("Bearer {SECRET}-grok-cli-work"));
}

#[tokio::test]
async fn an_effort_suffixed_model_differs_only_by_reasoning_effort() {
    let s = signed_in().await;
    s.mock.on("/v1/responses", [responses_stream()]);
    let mut body = codex_body("grok-4.5-high");
    body["reasoning"] = json!({"summary": "auto"});
    let (r, got) = send_body(&s, "openai-responses", "grok-cli/grok-4.5-high", body.clone(), &[]).await;
    let mut want = body;
    want["model"] = json!("grok-4.5");
    want["reasoning"]["effort"] = json!("high");
    assert_eq!(got.json(), want, "the client's own reasoning.summary stays");
    assert_eq!(got.body, serde_json::to_vec(&want).unwrap(), "field order is the client's");
    assert_eq!(r.attempts[0].forced, [("reasoning.effort".to_owned(), json!("high"))]);
    assert_eq!(got.headers["x-grok-model-override"], "grok-4.5");
}
