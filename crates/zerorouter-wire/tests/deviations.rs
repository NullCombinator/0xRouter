//! The deliberate deviations from 9router (T044, research R4): one test per row, each
//! asserting 0router's behaviour on the bundled styles. The oracle differences these rows
//! cause are listed in `tests/parity/deviations.toml`.

mod oracle;

use oracle::bundled;
use serde_json::{Value, json};
use zerorouter_wire::codec::response::{self, ForClient};
use zerorouter_wire::codec::{CodecError, request};

/// A `client`-style body sent to a `wire`-style provider, as the engine's translated path
/// encodes it.
fn translate(client: &str, wire: &str, body: &Value) -> Result<Value, CodecError> {
    let (c, w) = (bundled(client), bundled(wire));
    let mut ir = request::decode(&c, body).unwrap();
    ir.model = "m1".into();
    request::encode(&ir, &w, &c.id).map(|e| e.body)
}

fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|v| strings(v, out)),
        Value::Object(o) => o.values().for_each(|v| strings(v, out)),
        _ => {}
    }
}

fn chat_body() -> Value {
    json!({
        "model": "m",
        "messages": [
            { "role": "system", "content": "be brief" },
            { "role": "user", "content": "hi" }
        ]
    })
}

const WIRES: [&str; 4] = ["openai-chat", "anthropic-messages", "openai-responses", "gemini"];

#[test]
fn no_claude_code_system_prompt() {
    for client in ["openai-chat", "openai-responses", "gemini"] {
        let body = match client {
            "openai-chat" => chat_body(),
            "openai-responses" => json!({ "model": "m", "instructions": "be brief", "input": "hi" }),
            _ => {
                json!({ "systemInstruction": { "parts": [{ "text": "be brief" }] }, "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }] })
            }
        };
        let out = translate(client, "anthropic-messages", &body).unwrap();
        let mut got = Vec::new();
        strings(&out["system"], &mut got);
        assert_eq!(got, ["be brief"], "{client}: the system prompt is the client's own: {}", out["system"]);
        assert!(!out.to_string().contains("Claude Code"), "{out}");
    }
}

#[test]
fn response_format_goes_to_the_native_field_or_the_target_is_skipped() {
    let schema = json!({ "type": "object", "properties": { "a": { "type": "string" } } });
    let mut body = chat_body();
    body["response_format"] = json!({ "type": "json_schema", "json_schema": { "name": "r", "schema": schema } });

    let chat = translate("openai-chat", "openai-chat", &body).unwrap();
    assert_eq!(chat["response_format"], body["response_format"]);
    let responses = translate("openai-chat", "openai-responses", &body).unwrap();
    assert_eq!(responses["text"]["format"]["schema"], schema);
    let gemini = translate("openai-chat", "gemini", &body).unwrap();
    assert_eq!(gemini["generationConfig"]["responseSchema"], schema);
    assert_eq!(gemini["generationConfig"]["responseMimeType"], "application/json");

    // The Messages wire has no structured-output field: no system text stands in for it.
    let err = translate("openai-chat", "anthropic-messages", &body).unwrap_err();
    assert!(matches!(err, CodecError::CannotCarry { .. }), "{err}");

    for out in [chat, responses, gemini] {
        let mut got = Vec::new();
        strings(&out, &mut got);
        assert!(!got.iter().any(|s| s.contains("JSON")), "no instruction text was added: {out}");
    }
}

#[test]
fn no_fingerprint_tools() {
    let mut body = chat_body();
    body["tools"] =
        json!([{ "type": "function", "function": { "name": "get_weather", "parameters": { "type": "object" } } }]);
    for wire in WIRES {
        let out = translate("openai-chat", wire, &body).unwrap();
        let mut names = Vec::new();
        let text = out.to_string();
        for fp in ["\"bash\"", "\"glob\"", "\"grep\"", "\"read\""] {
            assert!(!text.contains(fp), "{wire}: a fingerprint tool {fp} was added: {text}");
        }
        strings(&out, &mut names);
        assert_eq!(names.iter().filter(|s| *s == "get_weather").count(), 1, "{wire}: {text}");
    }
    let plain = translate("openai-chat", "openai-chat", &chat_body()).unwrap();
    assert!(plain.get("tools").is_none(), "no tools appear where the client sent none: {plain}");
}

#[test]
fn cache_control_is_kept_and_none_is_added() {
    let body = json!({
        "model": "m",
        "max_tokens": 10,
        "system": [{ "type": "text", "text": "be brief", "cache_control": { "type": "ephemeral" } }],
        "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi", "cache_control": { "type": "ephemeral" } }] }],
        "tools": [{ "name": "t", "input_schema": { "type": "object" } }]
    });
    let out = translate("anthropic-messages", "anthropic-messages", &body).unwrap();
    assert_eq!(out["system"][0]["cache_control"], json!({ "type": "ephemeral" }));
    assert_eq!(out["messages"][0]["content"][0]["cache_control"], json!({ "type": "ephemeral" }));
    assert!(out["tools"][0].get("cache_control").is_none(), "no 1h marker on the last tool: {out}");

    // Into Messages from a client that sent none: none appears.
    let out = translate("openai-chat", "anthropic-messages", &chat_body()).unwrap();
    assert!(!out.to_string().contains("cache_control"), "{out}");
}

#[test]
fn url_images_and_tool_errors_are_carried() {
    let body = json!({
        "model": "m",
        "max_tokens": 10,
        "messages": [
            { "role": "user", "content": [
                { "type": "text", "text": "what is this?" },
                { "type": "image", "source": { "type": "url", "url": "https://example.com/a.png" } }
            ] },
            { "role": "assistant", "content": [{ "type": "tool_use", "id": "t1", "name": "look", "input": {} }] },
            { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "not found", "is_error": true }] }
        ],
        "tools": [{ "name": "look", "input_schema": { "type": "object" } }]
    });
    let chat = translate("anthropic-messages", "openai-chat", &body).unwrap();
    let text = chat.to_string();
    assert!(text.contains("https://example.com/a.png"), "the URL image arrives: {text}");
    let tool = chat["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap();
    assert_eq!(tool["content"], "Error: not found", "the error flag becomes a readable prefix");

    let responses = translate("anthropic-messages", "openai-responses", &body).unwrap();
    assert!(responses.to_string().contains("https://example.com/a.png"), "{responses}");

    // A wire with its own error field keeps the flag there, and the text unchanged.
    let messages = translate("anthropic-messages", "anthropic-messages", &body).unwrap();
    assert_eq!(messages["messages"][2]["content"][0]["is_error"], true);
    assert_eq!(messages["messages"][2]["content"][0]["content"], "not found");
}

#[test]
fn signed_thinking_is_dropped_across_vendors_and_kept_within_one() {
    let body = json!({
        "model": "m",
        "max_tokens": 10,
        "messages": [
            { "role": "user", "content": "2+2?" },
            { "role": "assistant", "content": [
                { "type": "thinking", "thinking": "adding", "signature": "sig-anthropic" },
                { "type": "text", "text": "4" }
            ] },
            { "role": "user", "content": "and 3+3?" }
        ]
    });
    for wire in ["openai-chat", "openai-responses", "gemini"] {
        let out = translate("anthropic-messages", wire, &body).unwrap();
        assert!(!out.to_string().contains("sig-anthropic"), "{wire}: a foreign signature crossed: {out}");
        let mut got = Vec::new();
        strings(&out, &mut got);
        assert!(
            !got.iter().any(|s| s.contains("adding") && s.contains('4')),
            "{wire}: thinking was merged into the answer: {out}"
        );
    }
    let same = translate("anthropic-messages", "anthropic-messages", &body).unwrap();
    assert_eq!(same["messages"][1]["content"][0]["signature"], "sig-anthropic");
}

#[test]
fn gemini_client_tools_are_translated() {
    let body = json!({
        "contents": [
            { "role": "user", "parts": [{ "text": "weather in Oslo?" }] },
            { "role": "model", "parts": [{ "functionCall": { "name": "get_weather", "args": { "city": "Oslo" } } }] },
            { "role": "user", "parts": [{ "functionResponse": { "name": "get_weather", "response": { "result": "rain" } } }] }
        ],
        "tools": [{ "functionDeclarations": [{ "name": "get_weather", "parameters": { "type": "object" } }] }]
    });
    let chat = translate("gemini", "openai-chat", &body).unwrap();
    assert_eq!(chat["tools"][0]["function"]["name"], "get_weather");
    let msgs = chat["messages"].as_array().unwrap();
    let call = &msgs[1]["tool_calls"][0];
    assert_eq!(call["function"]["name"], "get_weather");
    assert_eq!(
        serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap()).unwrap(),
        json!({ "city": "Oslo" })
    );
    assert_eq!(msgs[2]["role"], "tool");
    assert_eq!(msgs[2]["tool_call_id"], call["id"], "the result is paired with its call");

    let messages = translate("gemini", "anthropic-messages", &body).unwrap();
    let use_ = &messages["messages"][1]["content"][0];
    assert_eq!(use_["type"], "tool_use");
    assert_eq!(messages["messages"][2]["content"][0]["tool_use_id"], use_["id"]);
}

#[test]
fn non_stream_second_hop_is_written_in_the_clients_style() {
    let claude = json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "m1",
        "content": [{ "type": "text", "text": "hello" }],
        "stop_reason": "end_turn", "usage": { "input_tokens": 5, "output_tokens": 1 }
    });
    let wire = bundled("anthropic-messages");

    let ForClient::Rebuilt { body, .. } =
        response::for_client(&bundled("openai-responses"), &wire, &claude, 1_700_000_000).unwrap()
    else {
        panic!("across styles the body is rebuilt")
    };
    assert_eq!(body["object"], "response", "{body}");
    assert!(body.get("choices").is_none(), "not an OpenAI Chat body: {body}");
    assert_eq!(body["output"][0]["content"][0]["text"], "hello");
    assert_eq!(body["usage"]["input_tokens"], 5);

    let ForClient::Rebuilt { body, .. } =
        response::for_client(&bundled("gemini"), &wire, &claude, 1_700_000_000).unwrap()
    else {
        panic!("across styles the body is rebuilt")
    };
    assert!(body.get("choices").is_none(), "not an OpenAI Chat body: {body}");
    assert_eq!(body["candidates"][0]["content"]["parts"][0]["text"], "hello");
    assert_eq!(body["candidates"][0]["finishReason"], "STOP");
    assert_eq!(body["usageMetadata"]["promptTokenCount"], 5);
}
