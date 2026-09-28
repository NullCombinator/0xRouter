//! Optimizer pass-through on the two minimal test styles (T146, research R27). A
//! same-style attempt forwards the client's body with named edits only; a cross-style
//! attempt drops what it can't place and says where, never what.

use serde_json::{Value, json};
use zerorouter_registry::validate::validate_style;
use zerorouter_wire::codec::request::{self, Edits};
use zerorouter_wire::codec::response::{self, ForClient};
use zerorouter_wire::codec::{CodecError, Style};

fn style(file: &str) -> Style {
    let path = format!("{}/tests/fixtures/{file}", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap();
    Style::compile(&validate_style(&src, &path).unwrap()).unwrap()
}

const SECRET: &str = "sk-optimizer-SECRET-value";

/// A mini body as an optimizer hop might send it: unknown keys at every level, and a
/// block type no mini template describes.
fn optimized() -> Value {
    json!({
        "model": "claude-x",
        "stream": false,
        "max_tokens": 64,
        "system": "be brief",
        "x_top": { "token": SECRET },
        "messages": [
            { "role": "user", "x_msg": 1, "content": [
                { "type": "text", "text": "hi", "x_part": { "ttl": "1h" } },
                { "type": "document", "source": { "type": "text", "data": "doc" } }
            ] },
            { "role": "assistant", "content": "hello" }
        ],
        "tools": [{ "name": "f", "input_schema": { "type": "object" }, "x_tool": true }],
        "tool_choice": { "type": "auto", "disable_parallel_tool_use": true }
    })
}

fn without_document(mut body: Value) -> Value {
    body["messages"][0]["content"].as_array_mut().unwrap().pop();
    body
}

#[test]
fn same_style_forwards_the_body_with_named_edits_only() {
    let m = style("mini-style.toml");
    let body = optimized();
    let req = request::decode(&m, &body).unwrap();
    assert_eq!(req.opaque.len(), 1, "the document block is opaque to mini");

    let out = request::forward(&body, &m, &Edits { model: Some("up-1"), stream: Some(true), include_usage: true }).unwrap();
    let mut expected = body.clone();
    expected["model"] = json!("up-1");
    expected["stream"] = json!(true);
    assert_eq!(out, expected, "mini has no usage switch, so include_usage changes nothing");

    let out = request::forward(&body, &m, &Edits::default()).unwrap();
    assert_eq!(out, body);
}

#[test]
fn forward_sets_the_usage_switch_of_a_chat_style() {
    let c = style("mini-chat-style.toml");
    let body = json!({ "model": "a", "stream": true, "store": false, "stream_options": { "x_keep": 1 },
                       "messages": [{ "role": "user", "content": "hi", "x_msg": 2 }] });
    let out = request::forward(&body, &c, &Edits { model: Some("b"), stream: None, include_usage: true }).unwrap();
    let mut expected = body.clone();
    expected["model"] = json!("b");
    expected["stream_options"]["include_usage"] = json!(true);
    assert_eq!(out, expected);
}

#[test]
fn decoder_lists_every_unread_key_by_path() {
    let m = style("mini-style.toml");
    let req = request::decode(&m, &optimized()).unwrap();
    assert_eq!(
        req.unplaced,
        [
            "x_top",
            "messages[0].content[0].x_part",
            "messages[0].x_msg",
            "tools[0].x_tool",
            "tool_choice.disable_parallel_tool_use",
        ]
    );
}

#[test]
fn cross_style_drops_unknown_keys_and_records_paths_not_values() {
    let (m, c) = (style("mini-style.toml"), style("mini-chat-style.toml"));
    let req = request::decode(&m, &without_document(optimized())).unwrap();
    assert!(req.opaque.is_empty());
    let enc = request::encode(&req, &c, "mini").unwrap();

    let sent = enc.body.to_string();
    for key in ["x_top", "x_msg", "x_part", "x_tool", "disable_parallel_tool_use"] {
        assert!(!sent.contains(key), "{key} in {sent}");
    }
    assert_eq!(enc.body["messages"][1], json!({ "role": "user", "content": "hi" }));

    let paths: Vec<&str> = enc.dropped.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, req.unplaced);
    assert!(enc.dropped.iter().all(|d| d.reason.contains("minichat")), "{:?}", enc.dropped);
    let recorded = serde_json::to_string(&enc.dropped).unwrap();
    assert!(!recorded.contains(SECRET) && !recorded.contains("1h"), "{recorded}");
    assert_eq!(serde_json::to_value(&enc.dropped[0]).unwrap().as_object().unwrap().len(), 2, "path and reason only");
}

#[test]
fn cross_style_still_refuses_content_it_cant_carry() {
    let (m, c) = (style("mini-style.toml"), style("mini-chat-style.toml"));
    let req = request::decode(&m, &optimized()).unwrap();
    let err = request::encode(&req, &c, "mini").unwrap_err();
    assert!(matches!(&err, CodecError::CannotCarry { part, .. } if part == "messages[0].content[1]"), "{err}");
}

#[test]
fn same_style_response_is_kept_as_received_and_usage_is_still_read() {
    let (m, c) = (style("mini-style.toml"), style("mini-chat-style.toml"));
    let body = json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "m1",
        "content": [{ "type": "text", "text": "hi", "x_part": 1 }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 5, "output_tokens": 2, "x_usage": 3 },
        "context_management": { "applied_edits": [] }
    });
    let ForClient::AsReceived { read: Some(r) } = response::for_client(&m, &m, &body, 0).unwrap() else {
        panic!("a same-style body goes out as received")
    };
    let u = r.usage.unwrap();
    assert_eq!((u.input, u.output), (Some(5), Some(2)));

    let ForClient::AsReceived { read: None } = response::for_client(&m, &m, &json!({ "odd": true }), 0).unwrap() else {
        panic!("a body 0router can't read still goes out as received")
    };

    let ForClient::Rebuilt { body: rebuilt, .. } = response::for_client(&c, &m, &body, 0).unwrap() else {
        panic!("a cross-style body is rebuilt")
    };
    assert_eq!(rebuilt["choices"][0]["message"]["content"], "hi");
    assert!(rebuilt.get("context_management").is_none());
}
