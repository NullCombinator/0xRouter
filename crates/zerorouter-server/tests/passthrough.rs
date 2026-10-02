//! What reaches the provider and what comes back unchanged (T048, T147; US1-7 to US1-10,
//! R27): prompt text byte-for-byte in native and translated pairs; unknown fields and
//! headers through in the client's own style and left out, recorded, across styles; the
//! provider's own answer back unchanged in the client's own style.

mod common;

use common::{SECRET, chat_whole, multi_plugin, server_with};
use serde_json::{Value, json};
use zerorouter_engine::testkit::{Received, Step};
use zerorouter_server::relay::REQUEST_ID;

/// Prompt text that a re-encoding would be likely to disturb.
const SYSTEM: &str = "You are terse.\r\n  Keep \"quotes\", \\backslashes\\, tabs\t and trailing spaces  ";
const USER: &str = "héllo 👋🏽 — \u{2028} zero-width\u{200b}joiner, <b>not html</b> & {\"json\": true}";
const TOOL_RESULT: &str = "{\"temp\": 21.5, \"unit\": \"°C\"}\n";

fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|v| strings(v, out)),
        Value::Object(o) => o.values().for_each(|v| strings(v, out)),
        _ => {}
    }
}

/// Every prompt string is some string value of the body the provider got, unchanged.
fn assert_prompt_intact(r: &Received, case: &str) {
    let mut got = Vec::new();
    strings(&r.json(), &mut got);
    for want in [SYSTEM, USER, TOOL_RESULT] {
        assert!(got.iter().any(|s| s == want), "{case}: {want:?} didn't arrive unchanged in {}", r.json());
    }
}

fn chat_prompt(model: &str) -> Value {
    json!({
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": USER},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "weather", "arguments": "{\"city\":\"Oslo\"}"}}]},
            {"role": "tool", "tool_call_id": "call_1", "content": TOOL_RESULT},
        ],
        "tools": [{"type": "function", "function": {"name": "weather", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}],
    })
}

fn messages_prompt(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 64,
        "system": SYSTEM,
        "messages": [
            {"role": "user", "content": USER},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "weather", "input": {"city": "Oslo"}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": TOOL_RESULT}]},
        ],
        "tools": [{"name": "weather", "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}}}],
    })
}

fn messages_whole() -> Step {
    Step::json(
        200,
        json!({"id": "msg_up", "type": "message", "role": "assistant", "model": "m-messages", "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn", "usage": {"input_tokens": 5, "output_tokens": 1}}),
    )
}

#[tokio::test]
async fn prompt_text_reaches_the_provider_byte_for_byte() {
    let s = server_with(|m| vec![multi_plugin(m)]).await;
    let c = reqwest::Client::new();
    let chat =
        |body: Value| c.post(format!("{}/v1/chat/completions", s.base)).bearer_auth(&s.key).body(body.to_string());
    let messages = |body: Value| {
        c.post(format!("{}/v1/messages", s.base))
            .header("x-api-key", &s.key)
            .header("anthropic-version", "2023-06-01")
            .body(body.to_string())
    };

    let cases = [
        ("chat → chat (native)", chat(chat_prompt("mockco/m1")), chat_whole()),
        ("messages → messages (native)", messages(messages_prompt("multi/m-messages")), messages_whole()),
        ("messages → chat (translated)", messages(messages_prompt("mockco/m1")), chat_whole()),
        ("chat → messages (translated)", chat(chat_prompt("multi/m-messages")), messages_whole()),
    ];
    for (i, (case, req, reply)) in cases.into_iter().enumerate() {
        s.mock.push([reply]);
        let r = req.send().await.unwrap();
        assert_eq!(r.status(), 200, "{case}: {}", r.text().await.unwrap());
        assert_prompt_intact(&s.mock.received()[i], case);
    }
}

#[tokio::test]
async fn same_style_passes_unknown_fields_and_headers_but_never_the_floor() {
    let s = server_with(|_| Vec::new()).await;
    s.mock.push([chat_whole()]);
    let mut body = chat_prompt("mockco/m1");
    body["x_optimizer"] = json!({"compressed": true, "ratio": 0.4});
    body["messages"][1]["x_cache_hint"] = json!("keep");
    let r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .header("x-headroom-session", "hs-1")
        .header("openai-beta", "assistants=v2")
        .header("x-api-key", &s.key)
        .header("cookie", "sid=abc")
        .header("x-0router-debug", "1")
        .header("keep-alive", "timeout=5")
        .header("accept-encoding", "br")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    let got = &s.mock.received()[0];
    let b = got.json();
    assert_eq!(b["x_optimizer"], body["x_optimizer"], "an unknown top-level field goes through");
    assert_eq!(b["messages"][1]["x_cache_hint"], "keep", "an unknown message field goes through");
    let h = &got.headers;
    assert_eq!(h["x-headroom-session"], "hs-1");
    assert_eq!(h["openai-beta"], "assistants=v2");
    assert_eq!(h["authorization"], format!("Bearer {SECRET}"), "the account's secret replaces the access key");
    for name in ["x-api-key", "cookie", "x-0router-debug", "keep-alive"] {
        assert!(!h.contains_key(name), "{name} went upstream");
    }
    assert_ne!(
        h.get("accept-encoding").map(|v| v.as_bytes()),
        Some(&b"br"[..]),
        "the client's accept-encoding stays behind"
    );
    assert!(
        h.values().all(|v| !v.as_bytes().windows(s.key.len()).any(|w| w == s.key.as_bytes())),
        "the access key never goes upstream"
    );
}

#[tokio::test]
async fn cross_style_leaves_out_unknown_fields_and_headers_and_records_the_paths() {
    let s = server_with(|_| Vec::new()).await;
    s.mock.push([chat_whole()]);
    let mut body = messages_prompt("mockco/m1");
    body["x_optimizer"] = json!({"compressed": true});
    body["messages"][0]["x_cache_hint"] = json!("keep");
    let r = reqwest::Client::new()
        .post(format!("{}/v1/messages", s.base))
        .header("x-api-key", &s.key)
        .header("anthropic-version", "2023-06-01")
        .header("x-headroom-session", "hs-1")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();

    let got = &s.mock.received()[0];
    let sent = String::from_utf8_lossy(&got.body);
    assert!(!sent.contains("x_optimizer") && !sent.contains("x_cache_hint") && !sent.contains("compressed"), "{sent}");
    assert!(!got.headers.contains_key("x-headroom-session"), "an undeclared header crossed styles");
    assert!(!got.headers.contains_key("anthropic-version"), "a client-style header crossed styles");

    let rec = s.engine.records.get(&id).unwrap();
    let paths: Vec<&str> = rec.attempts[0].dropped.iter().map(|d| d.path.as_str()).collect();
    assert!(paths.iter().any(|p| p.contains("x_optimizer")), "{paths:?}");
    assert!(paths.iter().any(|p| p.contains("messages") && p.contains("x_cache_hint")), "{paths:?}");
    let dump = serde_json::to_string(&rec.attempts[0].dropped).unwrap();
    assert!(!dump.contains("compressed") && !dump.contains("keep\""), "the record holds paths, not values: {dump}");
}

#[tokio::test]
async fn same_style_answers_come_back_unchanged() {
    let s = server_with(|m| vec![multi_plugin(m)]).await;
    let c = reqwest::Client::new();

    // Whole: the provider's own bytes, unknown fields included.
    let whole = json!({"id": "up-9", "object": "chat.completion", "model": "m1", "x_provider_extra": {"region": "eu"}, "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi", "x_annotation": 1}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 5, "completion_tokens": 1, "total_tokens": 6}});
    s.mock.push([Step::json(200, whole.clone())]);
    let r = c
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .body(chat_prompt("mockco/m1").to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.bytes().await.unwrap(), whole.to_string().as_bytes(), "the body as the provider sent it");

    // Streamed: every event keeps its name and payload, including ones 0router can't read.
    let events: Vec<(Option<&str>, Value)> = vec![
        (
            Some("message_start"),
            json!({"type": "message_start", "message": {"id": "msg_up", "type": "message", "role": "assistant", "model": "m-messages", "content": [], "usage": {"input_tokens": 5, "output_tokens": 0}, "x_extra": true}}),
        ),
        (Some("x_vendor_event"), json!({"type": "x_vendor_event", "note": "unknown to 0router"})),
        (
            Some("content_block_start"),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            Some("content_block_delta"),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}}),
        ),
        (Some("content_block_stop"), json!({"type": "content_block_stop", "index": 0})),
        (
            Some("message_delta"),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
        ),
        (Some("message_stop"), json!({"type": "message_stop"})),
    ];
    let expected: String = events.iter().map(|(n, v)| format!("event: {}\ndata: {v}\n\n", n.unwrap())).collect();
    s.mock.push([Step::sse(&events, false)]);
    let mut body = messages_prompt("multi/m-messages");
    body["stream"] = json!(true);
    let r = c
        .post(format!("{}/v1/messages", s.base))
        .header("x-api-key", &s.key)
        .header("anthropic-version", "2023-06-01")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    assert_eq!(r.text().await.unwrap(), expected);
    let rec = s.engine.records.get(&id).unwrap();
    let u = rec.usage.unwrap();
    assert_eq!((u.input, u.output), (Some(5), Some(1)), "usage is still read from the relayed stream");
}

/// Fallback crossing styles: the first, same-style attempt gets the unknown field and
/// header and fails with a 503; the second, cross-style attempt gets neither and records
/// what it dropped.
#[tokio::test]
async fn a_fallback_across_styles_drops_what_the_first_attempt_carried() {
    let s = server_with(|m| {
        let plugin = |id: &str, path: &str, wire: &str, extra: &str| {
            format!(
                "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"{wire}\"\n{extra}[[models]]\nid = \"m1\"\n",
                m.url(path)
            )
        };
        let auth = "auth = { header = \"x-api-key\", scheme = \"raw\" }\n";
        vec![
            ("same", plugin("same", "/same/chat/completions", "openai-chat", "")),
            ("cross", plugin("cross", "/cross/messages", "anthropic-messages", auth)),
        ]
    })
    .await;
    let config = "allow_private_endpoints = true\n[[unified_model]]\nname = \"u\"\nmembers = [{ provider = \"same\", model = \"m1\" }, { provider = \"cross\", model = \"m1\" }]\n";
    std::fs::write(s.home().join("config.toml"), config).unwrap();
    let r = zerorouter_server::operator::handle(&s.engine, &json!({"op": "reload"})).await;
    assert_eq!(r["ok"], true, "{r}");
    s.mock.respond(|r: &Received| {
        if r.path_and_query.starts_with("/same") {
            Step::json(503, json!({"error": {"message": "overloaded"}}))
        } else {
            common::reply_by_wire(r)
        }
    });

    let mut body = chat_prompt("u");
    body["x_optimizer"] = json!({"compressed": true});
    let r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .header("x-headroom-session", "hs-1")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();

    let got = s.mock.received();
    let first = got.iter().find(|g| g.path_and_query.starts_with("/same")).expect("the same-style attempt");
    assert_eq!(first.json()["x_optimizer"], body["x_optimizer"]);
    assert_eq!(first.headers["x-headroom-session"], "hs-1");
    let second = got.iter().find(|g| g.path_and_query.starts_with("/cross")).expect("the cross-style attempt");
    assert!(!String::from_utf8_lossy(&second.body).contains("x_optimizer"));
    assert!(!second.headers.contains_key("x-headroom-session"), "an undeclared header crossed styles");

    let rec = s.engine.records.get(&id).unwrap();
    let last = rec.attempts.iter().rfind(|a| a.provider == "cross").unwrap();
    assert!(last.dropped.iter().any(|d| d.path.contains("x_optimizer")), "{:?}", last.dropped);
    assert!(rec.attempts.iter().filter(|a| a.provider == "same").all(|a| a.dropped.is_empty()));
}
