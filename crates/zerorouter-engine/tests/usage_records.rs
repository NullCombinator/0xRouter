//! Usage in records and client answers (T100, SC-004): each chosen provider's wire and usage
//! shape (anthropic and opencode-zen on Messages, openrouter and opencode-go on Chat,
//! opencode on Responses) × each client style, whole and streamed. The record keeps the
//! provider's numbers as reported; the client reads the same counts in its own style.

mod common;

use common::{Setup, drain, request, settled, setup};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::{self, Answer, Piece};
use zerorouter_engine::testkit::{MockUpstream, Received, Step};
use zerorouter_registry::schema::{Framing, InputSemantics};
use zerorouter_wire::codec::response::{self, ForClient};
use zerorouter_wire::ir::{Event, Usage};
use zerorouter_wire::stream::{Framer, StreamReader, StreamWriter};

fn plugin(mock: &MockUpstream) -> Vec<(&'static str, String)> {
    let toml = format!(
        r#"schema = 2
id = "multi"
category = "apikey"
[auth]
kind = "apikey"
[[endpoints.text]]
url = "{chat}"
wire = "openai-chat"
[[endpoints.text]]
url = "{messages}"
wire = "anthropic-messages"
headers = {{ "anthropic-version" = "2023-06-01" }}
auth = {{ header = "x-api-key", scheme = "raw" }}
[[endpoints.text]]
url = "{responses}"
wire = "openai-responses"
[[models]]
id = "m-chat"
wires = ["openai-chat"]
[[models]]
id = "m-messages"
wires = ["anthropic-messages"]
[[models]]
id = "m-responses"
wires = ["openai-responses"]
"#,
        chat = mock.url("/multi/chat/completions"),
        messages = mock.url("/multi/messages"),
        responses = mock.url("/multi/responses"),
    );
    vec![("multi", toml)]
}

/// Every wire reports 100 uncached input, 200 cache-read and 20 output tokens; Messages
/// and Chat (OpenRouter's `cache_write_tokens`) also 30 cache-write. Responses has no
/// cache-write count.
fn reply(r: &Received) -> Step {
    let stream = r.json()["stream"] == Value::Bool(true);
    let path = r.path_and_query.as_str();
    let ev = |name: &'static str, v: Value| (Some(name), v);
    if path.contains("/messages") {
        let usage = json!({"input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 200, "cache_creation_input_tokens": 30});
        if !stream {
            return Step::json(
                200,
                json!({"id": "msg_up", "type": "message", "role": "assistant", "model": "m-messages", "content": [{"type": "text", "text": "Hello"}], "stop_reason": "end_turn", "stop_sequence": null, "usage": usage}),
            );
        }
        let start = json!({"input_tokens": 100, "output_tokens": 1, "cache_read_input_tokens": 200, "cache_creation_input_tokens": 30});
        return Step::sse(
            &[
                ev(
                    "message_start",
                    json!({"type": "message_start", "message": {"id": "msg_up", "type": "message", "role": "assistant", "model": "m-messages", "content": [], "stop_reason": null, "usage": start}}),
                ),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}),
                ),
                ev("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 20}}),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ],
            false,
        );
    }
    if path.contains("/responses") {
        let usage = json!({"input_tokens": 300, "output_tokens": 20, "total_tokens": 320, "input_tokens_details": {"cached_tokens": 200}});
        let item = json!({"type": "message", "id": "msg_up", "status": "completed", "role": "assistant", "content": [{"type": "output_text", "text": "Hello", "annotations": []}]});
        let done = json!({"id": "resp_up", "object": "response", "created_at": 1, "status": "completed", "model": "m-responses", "output": [item], "usage": usage});
        if !stream {
            return Step::json(200, done);
        }
        let started = json!({"id": "resp_up", "object": "response", "created_at": 1, "status": "in_progress", "model": "m-responses", "output": []});
        return Step::sse(
            &[
                ev("response.created", json!({"type": "response.created", "sequence_number": 0, "response": started})),
                ev(
                    "response.output_item.added",
                    json!({"type": "response.output_item.added", "sequence_number": 1, "output_index": 0, "item": {"type": "message", "id": "msg_up", "status": "in_progress", "role": "assistant", "content": []}}),
                ),
                ev(
                    "response.output_text.delta",
                    json!({"type": "response.output_text.delta", "sequence_number": 2, "item_id": "msg_up", "output_index": 0, "content_index": 0, "delta": "Hello"}),
                ),
                ev(
                    "response.output_item.done",
                    json!({"type": "response.output_item.done", "sequence_number": 3, "output_index": 0, "item": item}),
                ),
                ev("response.completed", json!({"type": "response.completed", "sequence_number": 4, "response": done})),
            ],
            false,
        );
    }
    let usage = json!({"prompt_tokens": 330, "completion_tokens": 20, "total_tokens": 350, "prompt_tokens_details": {"cached_tokens": 200, "cache_write_tokens": 30}});
    if !stream {
        return Step::json(
            200,
            json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "m-chat", "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hello"}, "finish_reason": "stop"}], "usage": usage}),
        );
    }
    let chunk = |delta: Value, finish: Value| {
        (
            None,
            json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m-chat", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}),
        )
    };
    Step::sse(
        &[
            chunk(json!({"role": "assistant", "content": "Hello"}), Value::Null),
            chunk(json!({}), json!("stop")),
            (
                None,
                json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m-chat", "choices": [], "usage": usage}),
            ),
        ],
        true,
    )
}

fn client_body(style: &str, target: &str, stream: bool) -> Value {
    match style {
        "openai-chat" => {
            json!({"model": target, "stream": stream, "stream_options": {"include_usage": true}, "messages": [{"role": "user", "content": "hi"}]})
        }
        "anthropic-messages" => {
            json!({"model": target, "max_tokens": 64, "stream": stream, "messages": [{"role": "user", "content": "hi"}]})
        }
        "openai-responses" => json!({"model": target, "stream": stream, "input": "hi"}),
        _ => json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}),
    }
}

/// The usage the client reads from its answer, parsed back with its own style.
async fn client_usage(s: &Setup, style: &str, body: &Value, answer: Answer) -> Usage {
    let client = s.engine.snapshot().style(style).unwrap().clone();
    match answer {
        Answer::Whole { raw, answer, .. } => {
            let out = match *answer {
                ForClient::AsReceived { .. } => serde_json::from_slice(&raw).unwrap(),
                ForClient::Rebuilt { body, .. } => body,
            };
            response::decode(&client, &out)
                .unwrap_or_else(|e| panic!("{style}: {e}: {out:#}"))
                .usage
                .unwrap_or_default()
        }
        Answer::Events { rx, forced: true } => {
            attempt::collect(&client, body, rx).await.unwrap().usage.unwrap_or_default()
        }
        Answer::Events { mut rx, forced: false } => {
            let framing = client.text().map_or(Framing::SseData, |t| t.framing);
            let mut w = StreamWriter::new(&client, body, "rq_t", "", 0).unwrap();
            let mut text = String::new();
            for p in drain(&mut rx).await {
                match p {
                    Piece::Event(ev) => text += &w.write(&ev),
                    Piece::Frame(f, events) => {
                        w.observe(&events);
                        text += &f.to_bytes(framing).unwrap_or_default();
                    }
                    Piece::Restart => text += &w.restart(),
                }
            }
            text += &w.end();
            let mut framer = Framer::new(framing);
            let mut frames = framer.feed(text.as_bytes());
            frames.extend(framer.finish());
            let mut reader = StreamReader::new(&client).unwrap();
            let mut u = Usage::default();
            let mut events: Vec<Event> =
                frames.iter().flat_map(|f| reader.read(f).unwrap_or_else(|e| panic!("{style}: {e}\n{text}"))).collect();
            events.extend(reader.finish());
            for e in events {
                if let Event::Usage(x) = e {
                    u.merge(x);
                }
            }
            u
        }
        Answer::Media(_) => panic!("a text request got media"),
    }
}

#[tokio::test]
async fn recorded_and_client_usage_equal_the_reported_numbers() {
    let s = setup(plugin, &[("multi", "main")], "").await;
    s.mock.respond(reply);
    // (model, reported input, its semantics, cache-write reported)
    let wires = [
        ("m-messages", 100, InputSemantics::ExcludesCache, Some(30)),
        ("m-chat", 330, InputSemantics::IncludesCache, Some(30)),
        ("m-responses", 300, InputSemantics::IncludesCache, None),
    ];
    let mut failures = Vec::new();
    for (model, input, semantics, write) in wires {
        for style in ["openai-chat", "anthropic-messages", "openai-responses", "gemini"] {
            for stream in [false, true] {
                let target = format!("multi/{model}");
                let body = client_body(style, &target, stream);
                let mut req = request(&s, style, &target, body.clone(), "ak_test", CancellationToken::new());
                req.stream = stream;
                let id = req.id.clone();
                let case = format!("{model} → {style}{}", if stream { " (stream)" } else { "" });
                let answer = match s.engine.text(s.engine.snapshot(), req).await {
                    Ok(a) => a,
                    Err(f) => {
                        failures.push(format!("{case}: failed: {f:?}"));
                        continue;
                    }
                };
                let seen = client_usage(&s, style, &body, answer).await;
                let rec = settled(&s, &id).await.usage;
                let rec = rec.unwrap_or_else(|| panic!("{case}: no usage recorded"));
                let want = (Some(input), Some(20), Some(200), write, semantics);
                let got = (rec.input, rec.output, rec.cache_read, rec.cache_write, rec.input_semantics);
                if got != want {
                    failures.push(format!("{case}: record {got:?}, want {want:?}"));
                }
                // The client sees the same prompt total (input + cache) whatever its style's
                // semantics; only Messages clients have a place for the cache-write count.
                let n = |v: Option<u64>| v.unwrap_or(0);
                let prompt = n(seen.input) + n(seen.cache_read) + n(seen.cache_write);
                let total = 100 + 200 + n(write);
                let write_seen = if style == "anthropic-messages" { seen.cache_write } else { write };
                if (prompt, seen.output, seen.cache_read, write_seen) != (total, Some(20), Some(200), write) {
                    failures.push(format!("{case}: client read {seen:?}, want prompt {total}, output 20, cache-read 200, cache-write {write:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
