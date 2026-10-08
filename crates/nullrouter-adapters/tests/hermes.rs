//! The hermes adapter's rules (spec 004, T026): echoed reasoning, images and attachments, and
//! the things it must never touch.

use std::borrow::Cow;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use nullrouter_adapter_kit::{Capabilities, Context, Direction};
use nullrouter_adapters::HarnessName;
use nullrouter_adapters::apply::ContentChange;
use nullrouter_adapters::record::AdapterOutcome;
use nullrouter_adapters::runner::AdapterRunner;
use serde_json::{Value, json};

fn ctx(provider: &str, same_style: bool) -> Context {
    Context {
        direction: Direction::Request,
        provider: provider.into(),
        target_style: if same_style { "openai-chat" } else { "anthropic-messages" }.into(),
        same_style,
        model: "m".into(),
        model_type: "text".into(),
        capabilities: Capabilities::default(),
        stream: false,
        attempt: 1,
    }
}

/// The body after hermes, and the changes it recorded.
fn run(provider: &str, same_style: bool, body: &Value) -> (Value, Vec<ContentChange>) {
    let r = AdapterRunner::builtin(&HarnessName::new("hermes").unwrap()).unwrap();
    let out = r.run_request(&ctx(provider, same_style), body);
    assert_eq!(out.run.outcome, AdapterOutcome::Ran, "{:?}", out.run);
    (out.body.into_owned(), out.run.changes)
}

fn paths(changes: &[ContentChange]) -> Vec<String> {
    let mut p: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
    p.sort();
    p
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R'];
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1];
const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff";
const WEBP: &[u8] = b"RIFF\x24\x00\x00\x00WEBPVP8 \x18\x00\x00\x00";

fn echoed() -> Value {
    json!({"messages": [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "content": "hello", "reasoning_content": "R1", "reasoning": "R2",
         "reasoning_details": [{"type": "reasoning.text", "text": "R3"}]},
        {"role": "user", "content": "more"}
    ]})
}

// (a)
#[test]
fn echoed_reasoning_is_removed_for_a_provider_that_rejects_it_on_a_same_style_attempt() {
    for provider in ["groq", "mistral", "cerebras"] {
        let (out, changes) = run(provider, true, &echoed());
        let m = &out["messages"][1];
        assert!(m.get("reasoning_content").is_none() && m.get("reasoning").is_none() && m.get("reasoning_details").is_none());
        assert_eq!(m["content"], "hello", "the rest of the message stays");
        assert_eq!(
            paths(&changes),
            ["messages[1].reasoning", "messages[1].reasoning_content", "messages[1].reasoning_details"]
        );
        assert!(changes.iter().all(|c| format!("{:?}/{:?}", c.kind, c.reason) == "Removed/TargetRejectsField"));
    }
}

// (b)
#[test]
fn echoed_reasoning_stays_for_a_provider_not_in_the_table() {
    let body = echoed();
    let r = AdapterRunner::builtin(&HarnessName::new("hermes").unwrap()).unwrap();
    let out = r.run_request(&ctx("openrouter", true), &body);
    assert!(matches!(out.body, Cow::Borrowed(_)), "no edit, no copy");
    assert!(out.run.changes.is_empty());
}

// (c)
#[test]
fn echoed_reasoning_is_left_to_the_encoder_on_a_cross_style_attempt() {
    let body = echoed();
    let r = AdapterRunner::builtin(&HarnessName::new("hermes").unwrap()).unwrap();
    let out = r.run_request(&ctx("groq", false), &body);
    assert!(matches!(out.body, Cow::Borrowed(_)));
    assert!(out.run.changes.is_empty());
}

#[test]
fn only_assistant_messages_lose_echoed_reasoning() {
    let body = json!({"messages": [{"role": "user", "content": "hi", "reasoning": "keep"}]});
    let (out, changes) = run("groq", true, &body);
    assert_eq!(out, body);
    assert!(changes.is_empty());
}

// (d)
#[test]
fn images_become_content_parts_whatever_form_they_come_in() {
    let body = json!({"messages": [{
        "role": "user",
        "content": "what are these",
        "images": [
            b64(PNG),
            {"data": b64(JPEG), "mime": "image/jpeg"},
            {"url": format!("data:image/gif;base64,{}", b64(GIF))},
            b64(WEBP)
        ]
    }]});
    let (out, changes) = run("openrouter", true, &body);
    let m = &out["messages"][0];
    assert!(m.get("images").is_none());
    let parts = m["content"].as_array().expect("content is now parts");
    assert_eq!(parts[0], json!({"type": "text", "text": "what are these"}));
    let url = |i: usize| parts[i]["image_url"]["url"].as_str().unwrap().to_owned();
    assert_eq!(parts.len(), 5);
    assert_eq!(url(1), format!("data:image/png;base64,{}", b64(PNG)));
    assert_eq!(url(2), format!("data:image/jpeg;base64,{}", b64(JPEG)));
    assert_eq!(url(3), format!("data:image/gif;base64,{}", b64(GIF)));
    assert_eq!(url(4), format!("data:image/webp;base64,{}", b64(WEBP)));
    assert!(parts[1..].iter().all(|p| p["type"] == "image_url"));
    assert_eq!(paths(&changes), ["messages[0].content", "messages[0].images"]);
    assert!(changes.iter().all(|c| format!("{:?}", c.reason) == "FormatConversion"));
}

#[test]
fn images_join_parts_that_are_already_there() {
    let body = json!({"messages": [{
        "role": "user",
        "content": [{"type": "text", "text": "look"}],
        "images": [b64(PNG)]
    }]});
    let (out, _) = run("openrouter", true, &body);
    let parts = out["messages"][0]["content"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["text"], "look");
}

#[test]
fn an_image_whose_type_cannot_be_told_stays_where_it_was() {
    let body = json!({"messages": [{"role": "user", "content": "x", "images": [b64(b"not an image at all")]}]});
    let r = AdapterRunner::builtin(&HarnessName::new("hermes").unwrap()).unwrap();
    let out = r.run_request(&ctx("openrouter", true), &body);
    assert!(matches!(out.body, Cow::Borrowed(_)), "nothing was converted, nothing deleted");
}

#[test]
fn an_image_that_converts_beside_one_that_does_not_leaves_only_the_second() {
    let odd = b64(b"not an image at all");
    let body = json!({"messages": [{"role": "user", "content": "x", "images": [b64(PNG), odd.clone()]}]});
    let (out, changes) = run("openrouter", true, &body);
    assert_eq!(out["messages"][0]["images"], json!([odd]));
    assert_eq!(out["messages"][0]["content"].as_array().unwrap().len(), 2);
    assert_eq!(paths(&changes), ["messages[0].content", "messages[0].images"]);
}

// (e)
#[test]
fn attachments_map_by_mime_type() {
    let pdf = b64(b"%PDF-1.7 test");
    let body = json!({"messages": [{
        "role": "user",
        "content": "read these",
        "experimental_attachments": [
            {"url": format!("data:image/png;base64,{}", b64(PNG)), "contentType": "image/png", "name": "a.png"},
            {"data": pdf.clone(), "mediaType": "application/pdf", "name": "doc.pdf"}
        ]
    }]});
    let (out, changes) = run("openrouter", true, &body);
    let m = &out["messages"][0];
    assert!(m.get("experimental_attachments").is_none());
    let parts = m["content"].as_array().unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[1]["type"], "image_url");
    assert_eq!(parts[2], json!({"type": "file", "file": {"file_data": format!("data:application/pdf;base64,{pdf}"), "filename": "doc.pdf"}}));
    assert_eq!(paths(&changes), ["messages[0].content", "messages[0].experimental_attachments"]);
}

// (f)
#[test]
fn a_type_no_model_reads_is_left_unconverted() {
    let zip = json!({"data": b64(b"PK\x03\x04"), "contentType": "application/zip", "name": "a.zip"});
    let body = json!({"messages": [{"role": "user", "content": "x", "attachments": [zip]}]});
    let r = AdapterRunner::builtin(&HarnessName::new("hermes").unwrap()).unwrap();
    let out = r.run_request(&ctx("openrouter", true), &body);
    assert!(matches!(out.body, Cow::Borrowed(_)));

    // Beside a readable one, it stays in the field.
    let png = json!({"data": b64(PNG), "contentType": "image/png", "name": "a.png"});
    let body = json!({"messages": [{"role": "user", "content": "x", "attachments": [png, zip]}]});
    let (out, _) = run("openrouter", true, &body);
    assert_eq!(out["messages"][0]["attachments"], json!([zip]));
}

#[test]
fn a_message_with_no_content_key_is_left_alone() {
    let body = json!({"messages": [{"role": "user", "images": [b64(PNG)]}]});
    let r = AdapterRunner::builtin(&HarnessName::new("hermes").unwrap()).unwrap();
    let out = r.run_request(&ctx("openrouter", true), &body);
    assert!(matches!(out.body, Cow::Borrowed(_)));
}

// (g)
#[test]
fn tool_calls_tools_and_tool_results_are_never_touched() {
    let tools = json!([{"type": "function", "function": {"name": "f", "parameters": {"type": "object"}}}]);
    let calls = json!([{"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]);
    let body = json!({
        "tools": tools,
        "messages": [
            {"role": "user", "content": "go", "images": [b64(PNG)]},
            {"role": "assistant", "content": null, "tool_calls": calls, "reasoning_content": "R"},
            {"role": "tool", "tool_call_id": "c1", "content": "result", "images": [b64(PNG)], "reasoning": "keep"}
        ]
    });
    let (out, _) = run("groq", true, &body);
    assert_eq!(out["tools"], tools);
    assert_eq!(out["messages"][1]["tool_calls"], calls);
    assert!(out["messages"][1].get("reasoning_content").is_none(), "the echoed field goes, the calls stay");
    assert_eq!(out["messages"][2], body["messages"][2], "a tool message is untouched");
}

#[test]
fn records_hold_paths_and_reasons_never_content() {
    let (_, changes) = run("groq", true, &echoed());
    assert!(!format!("{changes:?}").contains("R1"));
}

// ---- parity deviations (T030), tests/parity/deviations.toml ----

#[test]
fn deviation_hermes_images_are_converted_where_9router_deletes_them() {
    // modality.js removes an image the model can't read. hermes keeps every image: converted,
    // or left in its field when it can't be.
    let readable = json!({"messages": [{"role": "user", "content": "x", "images": [b64(PNG)]}]});
    let (out, _) = run("openrouter", true, &readable);
    assert!(out.to_string().contains(&b64(PNG)), "the image data is still in the request");

    let unreadable = json!({"messages": [{"role": "user", "content": "x", "images": [b64(b"opaque")]}]});
    let r = AdapterRunner::builtin(&HarnessName::new("hermes").unwrap()).unwrap();
    assert!(matches!(r.run_request(&ctx("openrouter", true), &unreadable).body, Cow::Borrowed(_)));
}

#[test]
fn deviation_echoed_reasoning_is_the_adapters_table_seeded_with_9routers_three() {
    use nullrouter_adapters::builtin::hermes::REJECTS_ECHOED_REASONING;
    let providers: Vec<&str> = REJECTS_ECHOED_REASONING.iter().map(|(p, _)| *p).collect();
    assert_eq!(providers, ["groq", "mistral", "cerebras"], "chosen providers are added only by the live check");
    for (_, fields) in REJECTS_ECHOED_REASONING {
        assert_eq!(*fields, ["reasoning_content", "reasoning", "reasoning_details"]);
    }
}
