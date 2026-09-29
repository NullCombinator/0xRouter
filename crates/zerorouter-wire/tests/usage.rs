//! Usage parity with 9router (T099, research R13): every `tests/fixtures/9router/usage`
//! case read with the bundled style, whole bodies through the response decoder and streams
//! through the wire's reader, compared with 9router's canonical usage. Canonical prompt
//! tokens include cache reads and writes; the IR's `input` excludes both.

mod oracle;

use oracle::{bundled, root, style_id};
use serde_json::{Value, json};
use zerorouter_wire::codec::response;
use zerorouter_wire::ir::{Event, Usage};
use zerorouter_wire::stream::{Frame, StreamReader};

fn whole(style: &str, body: &Value) -> Usage {
    response::decode(&bundled(style), body).unwrap().usage.unwrap_or_default()
}

/// Usage events merged in order, as the engine records them.
fn streamed(style: &str, events: &[Value]) -> Usage {
    let wire = bundled(style);
    let mut reader = StreamReader::new(&wire).unwrap();
    let mut u = Usage::default();
    let mut take = |evs: Vec<Event>| {
        for e in evs {
            if let Event::Usage(x) = e {
                u.merge(x);
            }
        }
    };
    for ev in events {
        take(reader.read(&Frame { event: ev["type"].as_str().map(str::to_owned), data: ev.to_string() }).unwrap());
    }
    take(reader.finish());
    u
}

/// The IR usage in 9router's canonical shape; an absent count is 0 there.
fn canonical(u: &Usage) -> Value {
    let n = |v: Option<u64>| v.unwrap_or(0);
    let prompt = n(u.input) + n(u.cache_read) + n(u.cache_write);
    let mut c = json!({
        "prompt_tokens": prompt,
        "completion_tokens": n(u.output),
        "total_tokens": prompt + n(u.output),
        "cached_tokens": n(u.cache_read),
        "cache_creation_input_tokens": n(u.cache_write),
    });
    if n(u.reasoning) > 0 {
        c["reasoning_tokens"] = json!(n(u.reasoning));
    }
    c
}

#[test]
fn usage_matches_the_9router_oracle() {
    let path = root().join("tests/fixtures/9router/usage/cases.json");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let cases = file["data"]["cases"].as_array().unwrap();
    assert!(cases.len() >= 10, "the usage oracle is short (run tools/gen-bundled/generate.mjs)");
    let mut failures = Vec::new();
    for c in cases {
        let style = style_id(c["style"].as_str().unwrap());
        let u = match c["kind"].as_str().unwrap() {
            "whole" => whole(style, &c["input"]),
            _ => streamed(style, c["input"].as_array().unwrap()),
        };
        let got = canonical(&u);
        if got != c["canonical"] {
            failures.push(format!("{}: got {got}, want {}", c["name"], c["canonical"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn chat(usage: Value) -> Value {
    json!({"id": "c1", "object": "chat.completion", "model": "m", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}], "usage": usage})
}

#[test]
fn responses_nested_cached_tokens_are_read() {
    let body = json!({"id": "resp_1", "object": "response", "created_at": 1, "status": "completed", "model": "m", "output": [{"type": "message", "id": "m1", "role": "assistant", "content": [{"type": "output_text", "text": "hi"}]}], "usage": {"input_tokens": 1000, "output_tokens": 5, "input_tokens_details": {"cached_tokens": 800}}});
    let u = whole("openai-responses", &body);
    assert_eq!((u.input, u.cache_read, u.output), (Some(200), Some(800), Some(5)));
}

#[test]
fn openrouter_cache_write_tokens_are_read() {
    // 9router reads only cached_tokens here; 0router also keeps the write (deliberate).
    let u = whole(
        "openai-chat",
        &chat(
            json!({"prompt_tokens": 1000, "completion_tokens": 9, "prompt_tokens_details": {"cached_tokens": 600, "cache_write_tokens": 300}}),
        ),
    );
    assert_eq!((u.input, u.cache_read, u.cache_write), (Some(100), Some(600), Some(300)));
}

#[test]
fn gemini_cache_and_thoughts_are_read() {
    let body = json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "hi"}]}, "finishReason": "STOP", "index": 0}], "usageMetadata": {"promptTokenCount": 500, "candidatesTokenCount": 80, "thoughtsTokenCount": 40, "cachedContentTokenCount": 120}, "modelVersion": "m", "responseId": "r1"});
    let u = whole("gemini", &body);
    assert_eq!((u.input, u.cache_read, u.output, u.reasoning), (Some(380), Some(120), Some(80), Some(40)));
}

#[test]
fn an_absent_count_is_not_reported() {
    let u = whole("openai-chat", &chat(json!({"prompt_tokens": 7, "completion_tokens": 4})));
    assert_eq!((u.cache_read, u.cache_write, u.reasoning), (None, None, None));
    let body = json!({"id": "c1", "object": "chat.completion", "model": "m", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}]});
    assert_eq!(response::decode(&bundled("openai-chat"), &body).unwrap().usage.unwrap_or_default(), Usage::default());
}

#[test]
fn messages_usage_reaches_a_chat_client_cache_inclusive() {
    let body = json!({"id": "msg_1", "type": "message", "role": "assistant", "model": "m", "content": [{"type": "text", "text": "hi"}], "stop_reason": "end_turn", "usage": {"input_tokens": 100, "output_tokens": 50, "cache_read_input_tokens": 200, "cache_creation_input_tokens": 30}});
    let r = response::decode(&bundled("anthropic-messages"), &body).unwrap();
    let out = response::encode(&bundled("openai-chat"), &r, 0).unwrap();
    assert_eq!(out["usage"]["prompt_tokens"], 330, "{out:#}");
    assert_eq!(out["usage"]["completion_tokens"], 50);
    assert_eq!(out["usage"]["total_tokens"], 380);
    assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 200);
}
