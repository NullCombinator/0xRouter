//! The generic codecs on two minimal test styles (T029): `mini` has explicit blocks and
//! content-part tools; `minichat` has implicit blocks, a tool-calls field and tool-role
//! messages. No real style is involved.

use serde_json::{Value, json};
use zerorouter_registry::validate::validate_style;
use zerorouter_wire::codec::{CodecError, Style, request, response};
use zerorouter_wire::ir::{BlockKind, Event, FinishReason, Part, ResultContent, Role, Usage};
use zerorouter_wire::stream::{Frame, Framer, StreamReader, StreamWriter};

fn style(file: &str) -> Style {
    let path = format!("{}/tests/fixtures/{file}", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap();
    let parsed = validate_style(&src, &path).unwrap_or_else(|e| panic!("{file} fails the style gate: {e:#?}"));
    Style::compile(&parsed).unwrap()
}

fn mini() -> Style {
    style("mini-style.toml")
}

fn chat() -> Style {
    style("mini-chat-style.toml")
}

fn mini_body() -> Value {
    json!({
        "model": "m1",
        "stream": false,
        "max_tokens": 100,
        "system": "be brief",
        "thinking": { "type": "enabled", "budget_tokens": 2000 },
        "messages": [
            { "role": "user", "content": [
                { "type": "text", "text": "weather?", "cache_control": { "type": "ephemeral" } },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAA" } }
            ] },
            { "role": "assistant", "content": [
                { "type": "text", "text": "checking" },
                { "type": "tool_use", "id": "t1", "name": "get_weather", "input": { "city": "Oslo" } }
            ] },
            { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": "rain", "is_error": false },
                { "type": "text", "text": "thanks" }
            ] }
        ],
        "tools": [{ "name": "get_weather", "description": "d", "input_schema": { "type": "object" } }],
        "tool_choice": { "type": "auto" },
        "metadata": { "user_id": "u" }
    })
}

#[test]
fn request_round_trips_in_its_own_style() {
    let s = mini();
    let body = mini_body();
    let req = request::decode(&s, &body).unwrap();
    assert!(req.opaque.is_empty(), "{:?}", req.opaque);
    assert_eq!(req.model, "m1");
    assert_eq!(req.system, vec![Part::text("be brief")]);
    assert_eq!(req.params.thinking.as_ref().and_then(|t| t.budget_tokens), Some(2000));
    assert_eq!(req.extra.get("metadata"), Some(&json!({ "user_id": "u" })));
    assert!(matches!(&req.messages[2].parts[0], Part::ToolResult { content: ResultContent::Text(t), .. } if t == "rain"));
    assert_eq!(request::encode(&req, &s, "mini").unwrap(), body);
}

#[test]
fn request_translates_between_layouts_and_back() {
    let (m, c) = (mini(), chat());
    let req = request::decode(&m, &mini_body()).unwrap();
    let out = request::encode(&req, &c, "mini").unwrap();
    assert_eq!(
        out,
        json!({
            "model": "m1",
            "stream": false,
            "max_tokens": 100,
            "reasoning_effort": "low",
            "messages": [
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": [
                    { "type": "text", "text": "weather?" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAA" } }
                ] },
                { "role": "assistant", "content": "checking", "tool_calls": [
                    { "id": "t1", "type": "function", "function": { "name": "get_weather", "arguments": "{\"city\":\"Oslo\"}" } }
                ] },
                { "role": "tool", "tool_call_id": "t1", "content": "rain" },
                { "role": "user", "content": "thanks" }
            ],
            "tools": [{ "type": "function", "function": { "name": "get_weather", "description": "d", "parameters": { "type": "object" } } }],
            "tool_choice": "auto"
        })
    );

    // Back again: the chat body decodes to the same conversation.
    let again = request::decode(&c, &out).unwrap();
    assert!(again.opaque.is_empty(), "{:?}", again.opaque);
    assert_eq!(again.system, req.system);
    let roles: Vec<Role> = again.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool, Role::User]);
    let back = request::encode(&again, &m, "minichat").unwrap();
    // The tool result and the text after it merge into one user turn again.
    assert_eq!(back["messages"][2]["content"][0]["tool_use_id"], "t1");
    assert_eq!(back["messages"][2]["content"][1], json!({ "type": "text", "text": "thanks" }));
    assert_eq!(back["thinking"], json!({ "type": "enabled", "budget_tokens": 1024 }));
}

#[test]
fn content_no_template_describes_is_never_dropped() {
    let (m, c) = (mini(), chat());
    let mut body = mini_body();
    body["messages"][0]["content"].as_array_mut().unwrap().push(json!({ "type": "document", "source": {} }));
    let req = request::decode(&m, &body).unwrap();
    assert_eq!(req.opaque.len(), 1);
    assert_eq!(req.opaque[0].at, "messages[0].content[2]");
    assert!(matches!(request::encode(&req, &c, "mini"), Err(CodecError::CannotCarry { .. })));

    // A mid-conversation system message has no place in a top-level system field.
    let body = json!({ "model": "m", "messages": [
        { "role": "user", "content": "a" }, { "role": "system", "content": "b" }
    ] });
    let req = request::decode(&c, &body).unwrap();
    let err = request::encode(&req, &m, "minichat").unwrap_err();
    assert!(matches!(&err, CodecError::CannotCarry { part, .. } if part == "system message"), "{err}");
}

fn frames(style: &Style, bytes: &str) -> Vec<Event> {
    let t = style.text.as_ref().unwrap();
    let mut framer = Framer::new(t.framing);
    let mut reader = StreamReader::new(style).unwrap();
    let mut got: Vec<Frame> = framer.feed(bytes.as_bytes());
    got.extend(framer.finish());
    let mut events: Vec<Event> = got.iter().flat_map(|f| reader.read(f).unwrap()).collect();
    events.extend(reader.finish());
    events
}

#[test]
fn stream_of_text_and_tool_call_translates_through_the_ir() {
    let (m, c) = (mini(), chat());
    let chunk = |delta: Value, finish: Value| {
        json!({ "id": "x1", "object": "chat.completion.chunk", "model": "m1",
                "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] })
    };
    let upstream: String = [
        chunk(json!({ "role": "assistant", "content": "" }), Value::Null),
        chunk(json!({ "content": "Hel" }), Value::Null),
        chunk(json!({ "content": "lo" }), Value::Null),
        chunk(json!({ "tool_calls": [{ "index": 0, "id": "c1", "type": "function", "function": { "name": "f", "arguments": "" } }] }), Value::Null),
        chunk(json!({ "tool_calls": [{ "index": 0, "function": { "arguments": "{\"a\":" } }] }), Value::Null),
        chunk(json!({ "tool_calls": [{ "index": 0, "function": { "arguments": "1}" } }] }), Value::Null),
        chunk(json!({}), json!("tool_calls")),
        json!({ "id": "x1", "object": "chat.completion.chunk", "model": "m1", "choices": [],
                "usage": { "prompt_tokens": 10, "completion_tokens": 4, "prompt_tokens_details": { "cached_tokens": 3 } } }),
    ]
    .iter()
    .map(|v| format!("data: {v}\n\n"))
    .chain(["data: [DONE]\n\n".to_owned()])
    .collect();

    let events = frames(&c, &upstream);
    assert_eq!(
        events,
        vec![
            Event::Preamble { id: Some("x1".into()), model: Some("m1".into()) },
            Event::BlockStart(BlockKind::Text),
            Event::TextDelta("Hel".into()),
            Event::TextDelta("lo".into()),
            Event::BlockStop,
            Event::BlockStart(BlockKind::ToolCall { id: "c1".into(), name: "f".into() }),
            Event::ToolArguments("{\"a\":".into()),
            Event::ToolArguments("1}".into()),
            Event::BlockStop,
            Event::Finish(FinishReason::ToolCalls),
            Event::Usage(Usage { input: Some(7), output: Some(4), cache_read: Some(3), ..Usage::default() }),
            Event::Done,
        ]
    );

    // Written as a mini stream, then read back by the mini reader.
    let mut w = StreamWriter::new(&m, &json!({}), "x1", "m1", 0).unwrap();
    let mut client: String = events.iter().map(|e| w.write(e)).collect();
    client.push_str(&w.end());
    assert!(client.starts_with("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_x1\""), "{client}");
    let names: Vec<&str> = client.lines().filter_map(|l| l.strip_prefix("event: ")).collect();
    assert_eq!(
        names,
        [
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ]
    );
    assert!(client.contains(r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}"#), "{client}");
    let back = frames(&m, &client);
    let text: String = back.iter().filter_map(|e| if let Event::TextDelta(t) = e { Some(t.as_str()) } else { None }).collect();
    assert_eq!(text, "Hello");
    assert!(back.contains(&Event::BlockStart(BlockKind::ToolCall { id: "c1".into(), name: "f".into() })));
    assert!(back.contains(&Event::Finish(FinishReason::ToolCalls)));
    assert_eq!(w.response().content[1], Part::ToolCall { id: "c1".into(), name: "f".into(), arguments: json!({ "a": 1 }), cache_control: None });
}

#[test]
fn chat_writer_adds_usage_only_when_asked_and_ends_with_done() {
    let c = chat();
    let events = [
        Event::TextDelta("hi".into()),
        Event::Usage(Usage { input: Some(5), output: Some(1), ..Usage::default() }),
        Event::Finish(FinishReason::Stop),
        Event::Done,
    ];
    let run = |req: Value| {
        let mut w = StreamWriter::new(&c, &req, "abc", "m1", 0).unwrap();
        events.iter().map(|e| w.write(e)).collect::<String>()
    };
    let plain = run(json!({}));
    assert!(!plain.contains("\"usage\""), "{plain}");
    assert!(plain.ends_with("data: [DONE]\n\n"));
    let with_usage = run(json!({ "stream_options": { "include_usage": true } }));
    assert!(with_usage.contains(r#""usage":{"completion_tokens":1,"prompt_tokens":5}"#) || with_usage.contains(r#""usage":{"prompt_tokens":5,"completion_tokens":1}"#), "{with_usage}");
    assert!(with_usage.contains("\"id\":\"chatcmpl-abc\""));
}

#[test]
fn non_stream_response_goes_through_the_ir() {
    let (m, c) = (mini(), chat());
    let upstream = json!({
        "id": "x9", "object": "chat.completion", "created": 1, "model": "m1",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok",
            "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "f", "arguments": "{\"a\":1}" } }] },
            "finish_reason": "tool_calls" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 2, "prompt_tokens_details": { "cached_tokens": 4 } }
    });
    let r = response::decode(&c, &upstream).unwrap();
    assert_eq!(r.finish, Some(FinishReason::ToolCalls));
    assert_eq!(r.usage.unwrap().input, Some(6));
    let out = response::encode(&m, &r, 0).unwrap();
    assert_eq!(
        out,
        json!({
            "id": "msg_x9", "type": "message", "role": "assistant", "model": "m1",
            "content": [
                { "type": "text", "text": "ok" },
                { "type": "tool_use", "id": "c1", "name": "f", "input": { "a": 1 } }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 6, "output_tokens": 2, "cache_read_input_tokens": 4 }
        })
    );
    // And back: chat reports input including the cache.
    let again = response::encode(&c, &response::decode(&m, &out).unwrap(), 7).unwrap();
    assert_eq!(again["usage"]["prompt_tokens"], 10);
    assert_eq!(again["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"], "{\"a\":1}");
    assert_eq!(again["id"], "chatcmpl-msg_x9");
}
