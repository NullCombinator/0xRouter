//! hermes through the engine (spec 004, T027): a key bound to `hermes` sends a multi-turn
//! session with a tool call, echoed reasoning and an image to scripted providers, streamed and
//! not. Asserts what each mock received, that every turn succeeds, and what the record lists.

mod common;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::*;
use nullrouter_engine::attempt::Answer;
use nullrouter_engine::keys::{HarnessName, Keys};
use nullrouter_engine::records::{AdapterOutcome, Outcome, RequestRecord};
use nullrouter_engine::testkit::Step;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R'];

async fn fleet() -> Setup {
    setup(
        |m| {
            vec![
                ("groq", chat_plugin(m, "groq", "")),
                ("openrouter", chat_plugin(m, "openrouter", "")),
                ("anthropic", messages_plugin(m, "anthropic")),
            ]
        },
        &[("groq", "a1"), ("openrouter", "a1"), ("anthropic", "a1")],
        // The ids are the real providers' (the reject table is keyed by them), so the test
        // plugins must replace the embedded ones or the requests would leave the machine.
        "[plugin_decisions]\ngroq = \"replace\"\nopenrouter = \"replace\"\nanthropic = \"replace\"\n",
    )
    .await
}

/// Issues a key bound to hermes (or to nothing) and returns its id.
fn key(s: &Setup, hermes: bool) -> String {
    let path = s._dir.path().join("keys.toml");
    let mut keys = Keys::load(&path).unwrap();
    let id = keys.issue(&format!("k{}", keys.iter().count()), None).unwrap().1.id.clone();
    if hermes {
        keys.set_adapter(&id, Some(HarnessName::new("hermes").unwrap())).unwrap();
    }
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();
    id
}

fn chat_reply(text: &str) -> Step {
    Step::json(
        200,
        json!({"id": "c1", "object": "chat.completion", "model": "m1",
               "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
               "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}}),
    )
}

fn tools() -> Value {
    json!([{"type": "function", "function": {"name": "lookup", "parameters": {"type": "object", "properties": {}}}}])
}

fn calls() -> Value {
    json!([{"id": "call_1", "type": "function", "function": {"name": "lookup", "arguments": "{}"}}])
}

/// The history after turn two: the model called a tool, hermes echoed its reasoning back.
fn with_tool(target: &str, stream: bool) -> Value {
    json!({"model": target, "stream": stream, "tools": tools(), "messages": [
        {"role": "user", "content": "find it"},
        {"role": "assistant", "content": null, "tool_calls": calls(),
         "reasoning_content": "SENTINEL-R1", "reasoning": "SENTINEL-R2",
         "reasoning_details": [{"type": "reasoning.text", "text": "SENTINEL-R3"}]},
        {"role": "tool", "tool_call_id": "call_1", "content": "found"},
        {"role": "user", "content": "now this picture", "images": [STANDARD.encode(PNG)]}
    ]})
}

/// One request, drained if it streams; the settled record.
async fn ask(s: &Setup, agent: &str, target: &str, body: Value) -> RequestRecord {
    let req = request(s, "openai-chat", target, body, agent, CancellationToken::new());
    let id = req.id.clone();
    if let Answer::Events { mut rx, .. } = s.engine.text(s.engine.snapshot(), req).await.unwrap_or_else(|e| panic!("{e:?}")) {
        drain(&mut rx).await;
    }
    settled(s, &id).await
}

fn last_sent(s: &Setup) -> Value {
    s.mock.received().last().expect("a request reached the mock").json()
}

fn changed(r: &RequestRecord) -> Vec<String> {
    let mut p: Vec<String> = r.attempts[0].adapter.as_ref().unwrap().changes.iter().map(|c| c.path.clone()).collect();
    p.sort();
    p
}

#[tokio::test]
async fn a_provider_that_rejects_echoed_reasoning_gets_the_turn_without_it() {
    let s = fleet().await;
    let id = key(&s, true);
    s.mock.push([chat_reply("one"), chat_reply("two")]);

    let first = ask(&s, &id, "groq/m1", chat_body("groq/m1", false)).await;
    assert_eq!(first.outcome, Outcome::Succeeded);

    let r = ask(&s, &id, "groq/m1", with_tool("groq/m1", false)).await;
    assert_eq!(r.outcome, Outcome::Succeeded);
    let sent = last_sent(&s);
    let text = sent.to_string();
    assert!(!text.contains("SENTINEL-R"), "no echoed reasoning upstream: {text}");
    assert_eq!(sent["tools"], tools());
    assert_eq!(sent["messages"][1]["tool_calls"], calls());
    assert_eq!(sent["messages"][2]["role"], "tool");
    assert_eq!(sent["messages"][2]["content"], "found");

    // The image turn: parts, not an `images` key.
    let last = &sent["messages"][3];
    assert!(last.get("images").is_none());
    assert_eq!(last["content"][0], json!({"type": "text", "text": "now this picture"}));
    assert_eq!(last["content"][1]["type"], "image_url");
    assert_eq!(last["content"][1]["image_url"]["url"], format!("data:image/png;base64,{}", STANDARD.encode(PNG)));

    assert_eq!(
        changed(&r),
        ["messages[1].reasoning", "messages[1].reasoning_content", "messages[1].reasoning_details", "messages[3].content", "messages[3].images"]
    );
    assert_eq!(r.attempts[0].adapter.as_ref().unwrap().outcome, AdapterOutcome::Ran);
    assert!(!format!("{:?}", r.attempts[0].adapter).contains("SENTINEL"), "the record holds no content");
}

#[tokio::test]
async fn a_provider_not_in_the_table_keeps_the_reasoning_and_still_gets_the_image_as_parts() {
    let s = fleet().await;
    let id = key(&s, true);
    s.mock.push([chat_reply("ok")]);
    let r = ask(&s, &id, "openrouter/m1", with_tool("openrouter/m1", false)).await;
    assert_eq!(r.outcome, Outcome::Succeeded);
    let sent = last_sent(&s);
    assert_eq!(sent["messages"][1]["reasoning_content"], "SENTINEL-R1");
    assert_eq!(sent["messages"][1]["reasoning_details"][0]["text"], "SENTINEL-R3");
    assert_eq!(sent["messages"][3]["content"][1]["type"], "image_url");
    assert_eq!(changed(&r), ["messages[3].content", "messages[3].images"]);
}

#[tokio::test]
async fn a_streamed_turn_goes_through_the_same_edits() {
    let s = fleet().await;
    let id = key(&s, true);
    s.mock.push([chat_chunks()]);
    let r = ask(&s, &id, "groq/m1", with_tool("groq/m1", true)).await;
    assert_eq!(r.outcome, Outcome::Succeeded);
    assert!(r.ttft_ms.is_some());
    let sent = last_sent(&s);
    assert_eq!(sent["stream"], true);
    assert!(!sent.to_string().contains("SENTINEL-R"));
    assert_eq!(sent["messages"][3]["content"][1]["type"], "image_url");
}

#[tokio::test]
async fn across_styles_the_adapter_leaves_reasoning_to_the_encoder_and_the_image_still_arrives() {
    let s = fleet().await;
    let id = key(&s, true);
    s.mock.push([Step::json(
        200,
        json!({"id": "msg_1", "type": "message", "role": "assistant", "model": "m1",
               "content": [{"type": "text", "text": "hi"}], "stop_reason": "end_turn",
               "usage": {"input_tokens": 3, "output_tokens": 1}}),
    )]);
    let r = ask(&s, &id, "anthropic/m1", with_tool("anthropic/m1", false)).await;
    assert_eq!(r.outcome, Outcome::Succeeded);
    // Only the image conversion: no reasoning removal on a cross-style attempt.
    assert_eq!(changed(&r), ["messages[3].content", "messages[3].images"]);
    let sent = last_sent(&s);
    let blocks = sent["messages"].as_array().unwrap().last().unwrap()["content"].as_array().unwrap().clone();
    assert!(blocks.iter().any(|b| b["type"] == "image"), "the image reached the Messages body as an image block: {sent}");
}

#[tokio::test]
async fn a_key_without_a_harness_goes_through_unchanged_and_records_no_adapter() {
    let s = fleet().await;
    let id = key(&s, false);
    s.mock.push([chat_reply("ok")]);
    let body = with_tool("groq/m1", false);
    let r = ask(&s, &id, "groq/m1", body.clone()).await;
    assert_eq!(r.outcome, Outcome::Succeeded);
    assert!(r.attempts[0].adapter.is_none());
    let sent = last_sent(&s);
    assert_eq!(sent["messages"][1]["reasoning_content"], "SENTINEL-R1");
    assert!(sent["messages"][3].get("images").is_some(), "no adapter, no conversion");
}
