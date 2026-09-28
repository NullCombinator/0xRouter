//! Every client style against every upstream text wire the chosen providers use, native and
//! translated, streamed and whole (T045, US1-1). Each answer must be valid in the client's
//! style, carry the request id, and report the provider's usage.

mod common;

use std::collections::BTreeSet;

use common::{multi_plugin, reply_by_wire, server_with};
use serde_json::{Value, json};
use zerorouter_engine::records::Outcome;
use zerorouter_server::relay::REQUEST_ID;

const STYLES: [&str; 4] = ["openai-chat", "anthropic-messages", "openai-responses", "gemini"];
const MODELS: [&str; 3] = ["multi/m-chat", "multi/m-messages", "multi/m-responses"];

fn request(c: &reqwest::Client, base: &str, key: &str, style: &str, model: &str, stream: bool) -> reqwest::RequestBuilder {
    match style {
        "openai-chat" => {
            let mut b = json!({"model": model, "stream": stream, "messages": [{"role": "user", "content": "hi"}]});
            if stream {
                b["stream_options"] = json!({"include_usage": true});
            }
            c.post(format!("{base}/v1/chat/completions")).bearer_auth(key).body(b.to_string())
        }
        "anthropic-messages" => c
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .body(json!({"model": model, "max_tokens": 64, "stream": stream, "messages": [{"role": "user", "content": "hi"}]}).to_string()),
        "openai-responses" => {
            c.post(format!("{base}/v1/responses")).bearer_auth(key).body(json!({"model": model, "stream": stream, "input": "hi"}).to_string())
        }
        "gemini" => {
            let op = if stream { "streamGenerateContent?alt=sse" } else { "generateContent" };
            c.post(format!("{base}/v1beta/models/{model}:{op}"))
                .header("x-goog-api-key", key)
                .body(json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}).to_string())
        }
        _ => unreachable!(),
    }
}

/// `(event name, data)` for each SSE frame; `[DONE]` comes back as a null payload.
fn frames(text: &str) -> Vec<(Option<String>, Value)> {
    let mut out = Vec::new();
    for block in text.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let mut name = None;
        let mut data = String::new();
        for line in block.lines() {
            if let Some(n) = line.strip_prefix("event: ") {
                name = Some(n.to_owned());
            } else if let Some(d) = line.strip_prefix("data: ") {
                data += d;
            }
        }
        let v = if data == "[DONE]" { Value::Null } else { serde_json::from_str(&data).unwrap_or_else(|e| panic!("{e}: {data}")) };
        out.push((name, v));
    }
    out
}

/// Text and `(input, output)` usage from a whole answer.
fn whole(style: &str, b: &Value) -> (String, (Value, Value)) {
    match style {
        "openai-chat" => (b["choices"][0]["message"]["content"].as_str().unwrap_or_default().into(), (b["usage"]["prompt_tokens"].clone(), b["usage"]["completion_tokens"].clone())),
        "anthropic-messages" => {
            assert_eq!(b["type"], "message");
            assert_eq!(b["stop_reason"], "end_turn");
            (b["content"][0]["text"].as_str().unwrap_or_default().into(), (b["usage"]["input_tokens"].clone(), b["usage"]["output_tokens"].clone()))
        }
        "openai-responses" => {
            assert_eq!(b["status"], "completed");
            let text = b["output"].as_array().unwrap().iter().filter(|i| i["type"] == "message").flat_map(|i| i["content"].as_array().unwrap().iter()).filter_map(|p| p["text"].as_str()).collect();
            (text, (b["usage"]["input_tokens"].clone(), b["usage"]["output_tokens"].clone()))
        }
        "gemini" => {
            assert_eq!(b["candidates"][0]["finishReason"], "STOP");
            let u = &b["usageMetadata"];
            (b["candidates"][0]["content"]["parts"][0]["text"].as_str().unwrap_or_default().into(), (u["promptTokenCount"].clone(), u["candidatesTokenCount"].clone()))
        }
        _ => unreachable!(),
    }
}

/// Checks the stream grammar and returns the text and usage.
fn streamed(style: &str, text: &str) -> (String, (Value, Value)) {
    let fs = frames(text);
    assert!(!fs.is_empty(), "empty stream");
    let mut out = String::new();
    let mut usage = (Value::Null, Value::Null);
    match style {
        "openai-chat" => {
            assert_eq!(fs.last().unwrap().1, Value::Null, "ends with [DONE]: {text}");
            let mut finished = false;
            for (_, d) in &fs[..fs.len() - 1] {
                assert_eq!(d["object"], "chat.completion.chunk", "{d}");
                for c in d["choices"].as_array().unwrap() {
                    out += c["delta"]["content"].as_str().unwrap_or_default();
                    finished |= c["finish_reason"] == "stop";
                }
                if d["usage"].is_object() {
                    usage = (d["usage"]["prompt_tokens"].clone(), d["usage"]["completion_tokens"].clone());
                }
            }
            assert!(finished, "a finish_reason: {text}");
        }
        "anthropic-messages" => {
            let names: Vec<&str> = fs.iter().map(|(n, _)| n.as_deref().expect("named events")).collect();
            assert_eq!(names.first(), Some(&"message_start"), "{text}");
            assert_eq!(names.last(), Some(&"message_stop"), "{text}");
            let (mut open, mut seen) = (BTreeSet::new(), BTreeSet::new());
            for (n, d) in &fs {
                assert_eq!(n.as_deref(), d["type"].as_str(), "event name matches type");
                let i = d["index"].as_u64();
                match n.as_deref().unwrap() {
                    "message_start" => usage.0 = d["message"]["usage"]["input_tokens"].clone(),
                    "content_block_start" => {
                        assert!(seen.insert(i.unwrap()), "index reused: {text}");
                        open.insert(i.unwrap());
                    }
                    "content_block_delta" => {
                        assert!(open.contains(&i.unwrap()), "delta before its block start: {text}");
                        out += d["delta"]["text"].as_str().unwrap_or_default();
                    }
                    "content_block_stop" => assert!(open.remove(&i.unwrap()), "stop without start"),
                    "message_delta" => {
                        assert_eq!(d["delta"]["stop_reason"], "end_turn");
                        usage.1 = d["usage"]["output_tokens"].clone();
                        // Across styles the input count is known only at the end, so it
                        // comes in message_delta, which Anthropic's usage there allows.
                        if usage.0.is_null() {
                            usage.0 = d["usage"]["input_tokens"].clone();
                        }
                    }
                    _ => {}
                }
            }
            assert!(open.is_empty(), "every block stopped");
        }
        "openai-responses" => {
            assert_eq!(fs.first().unwrap().1["type"], "response.created", "{text}");
            let last = &fs.last().unwrap().1;
            assert_eq!(last["type"], "response.completed", "{text}");
            for (n, d) in &fs {
                assert_eq!(n.as_deref(), d["type"].as_str(), "event name matches type");
                if d["type"] == "response.output_text.delta" {
                    out += d["delta"].as_str().unwrap();
                }
            }
            let u = &last["response"]["usage"];
            usage = (u["input_tokens"].clone(), u["output_tokens"].clone());
        }
        "gemini" => {
            for (_, d) in &fs {
                out += d["candidates"][0]["content"]["parts"][0]["text"].as_str().unwrap_or_default();
                if d["usageMetadata"].is_object() {
                    usage = (d["usageMetadata"]["promptTokenCount"].clone(), d["usageMetadata"]["candidatesTokenCount"].clone());
                }
            }
            assert!(fs.iter().any(|(_, d)| d["candidates"][0]["finishReason"] == "STOP"), "{text}");
        }
        _ => unreachable!(),
    }
    (out, usage)
}

#[tokio::test]
async fn every_style_over_every_wire_streamed_and_whole() {
    let s = server_with(|m| vec![multi_plugin(m)]).await;
    s.mock.respond(reply_by_wire);
    let c = reqwest::Client::new();
    let mut failures = Vec::new();
    for style in STYLES {
        for model in MODELS {
            for stream in [false, true] {
                let case = format!("{style} → {model} stream={stream}");
                let r = request(&c, &s.base, &s.key, style, model, stream).send().await.unwrap();
                let status = r.status();
                let id = r.headers().get(REQUEST_ID).map(|v| v.to_str().unwrap().to_owned());
                let body = r.text().await.unwrap();
                if status != 200 {
                    failures.push(format!("{case}: {status} {body}"));
                    continue;
                }
                let got = std::panic::catch_unwind(|| {
                    if stream {
                        streamed(style, &body)
                    } else {
                        whole(style, &serde_json::from_str(&body).unwrap())
                    }
                });
                match got {
                    Ok((text, usage)) if text == "Hello" && usage == (json!(5), json!(2)) => {}
                    Ok((text, usage)) => failures.push(format!("{case}: text {text:?} usage {usage:?}\n{body}")),
                    Err(_) => failures.push(format!("{case}: invalid body\n{body}")),
                }
                let Some(id) = id else {
                    failures.push(format!("{case}: no {REQUEST_ID}"));
                    continue;
                };
                // A stream's record settles after its last byte.
                for _ in 0..100 {
                    if s.engine.records.get(&id).unwrap().outcome != Outcome::InProgress {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                let rec = s.engine.records.get(&id).unwrap();
                let usage = rec.usage.map(|u| (u.input, u.output));
                if rec.outcome != Outcome::Succeeded || usage != Some((Some(5), Some(2))) || rec.style != style {
                    failures.push(format!("{case}: record {:?} usage {usage:?} style {}", rec.outcome, rec.style));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of 24 failed:\n\n{}", failures.len(), failures.join("\n\n"));
}
