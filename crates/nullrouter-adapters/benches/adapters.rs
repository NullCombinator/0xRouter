//! Adapter and guardrail cost (research R14, SC-010): the target is adapter plus guardrail at
//! most 5 ms p95 per request and at most 1 ms per stream event.
//!
//! - `adapters/guard_request/{10kb,100kb,1mb}`: the request guardrail over an anthropic-messages
//!   conversation of that size, after one removal (the last message).
//! - `adapters/guard_event`: the event guardrail over one anthropic-messages content-block delta.
//! - `adapters/selector_extract`: `messages[*].content[*].type` over a 1 MB body.
//! - `adapters/hermes_request`: hermes' selectors and request edits over an openai-chat body
//!   with images.
//!
//! `cargo bench -p nullrouter-adapters --bench adapters -- --save-baseline slice-004`

use std::hint::black_box;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use criterion::{Criterion, criterion_group, criterion_main};
use nullrouter_adapter_kit::{Capabilities, Context, Direction};
use nullrouter_adapters::builtin::hermes;
use nullrouter_adapters::guard;
use nullrouter_adapters::selector::{self, Selector};
use nullrouter_registry::validate::validate_style;
use nullrouter_wire::codec::{Style, request};
use serde_json::{Value, json};

/// A compiled client style, read the same way the guardrail tests read it.
fn style(id: &str) -> Style {
    let path = format!("{}/../../styles/bundled/{id}.toml", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).expect("the style file reads");
    let parsed = validate_style(&src, &path).unwrap_or_else(|e| panic!("{id} fails the style gate: {e:#?}"));
    Style::compile(&parsed).expect("the style compiles")
}

/// An anthropic-messages conversation of about `bytes` bytes: alternating turns of text blocks.
fn conversation(bytes: usize) -> Value {
    const TEXT: &str = "The quick brown fox jumps over the lazy dog. ";
    let mut messages = Vec::new();
    let mut size = 0;
    while size < bytes {
        let role = if messages.len().is_multiple_of(2) { "user" } else { "assistant" };
        let text = TEXT.repeat(20);
        size += text.len() + 40;
        messages.push(json!({"role": role, "content": [{"type": "text", "text": text}]}));
    }
    json!({"model": "claude-sonnet", "max_tokens": 1024, "stream": false, "messages": messages})
}

fn request_context(provider: &str, target_style: &str) -> Context {
    Context {
        direction: Direction::Request,
        provider: provider.to_owned(),
        target_style: target_style.to_owned(),
        same_style: true,
        model: "m1".to_owned(),
        model_type: "text".to_owned(),
        capabilities: Capabilities::default(),
        stream: false,
        attempt: 1,
    }
}

fn guard_request(c: &mut Criterion) {
    let style = style("anthropic-messages");
    let mut group = c.benchmark_group("adapters");
    for (name, size) in
        [("guard_request/10kb", 10_000), ("guard_request/100kb", 100_000), ("guard_request/1mb", 1_000_000)]
    {
        let body = conversation(size);
        let before = request::decode(&style, &body).expect("the conversation decodes");
        // One removal: the last message. Built outside the measurement.
        let mut after = body.clone();
        if let Some(messages) = after.get_mut("messages").and_then(Value::as_array_mut) {
            messages.pop();
        }
        group.bench_function(name, |b| {
            b.iter(|| guard::check_request(&style, &before, black_box(&after)));
        });
    }
    group.finish();
}

fn guard_event(c: &mut Criterion) {
    let style = style("anthropic-messages");
    let before = json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "text_delta", "text": "The quick brown fox jumps over the lazy dog."}
    });
    let after = before.clone();
    let mut group = c.benchmark_group("adapters");
    group.bench_function("guard_event", |b| {
        b.iter(|| guard::check_event_frame(&style, black_box(&before), black_box(&after)));
    });
    group.finish();
}

fn selector_extract(c: &mut Criterion) {
    let body = conversation(1_000_000);
    let selectors = [Selector::parse("messages[*].content[*].type").expect("a valid selector")];
    let mut group = c.benchmark_group("adapters");
    group.bench_function("selector_extract", |b| {
        b.iter(|| selector::extract(black_box(&body), &selectors));
    });
    group.finish();
}

fn hermes_request(c: &mut Criterion) {
    // A PNG signature followed by padding: enough for hermes to sniff the type.
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.resize(4096, 0);
    let image = json!({"data": STANDARD.encode(&png), "mime": "image/png"});
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(json!({
            "role": "user",
            "content": format!("turn {i}"),
            "images": [image.clone(), image.clone()]
        }));
        messages.push(json!({"role": "assistant", "content": "ok", "reasoning_content": "thinking"}));
    }
    let body = json!({"model": "m1", "stream": false, "messages": messages});
    let ctx = request_context("openai", "openai-chat");
    let selectors = hermes::request_selectors();

    let mut group = c.benchmark_group("adapters");
    group.bench_function("hermes_request", |b| {
        b.iter(|| {
            let found = selector::extract(black_box(&body), &selectors);
            black_box(found.len());
            hermes::on_request(&ctx, black_box(&body))
        });
    });
    group.finish();
}

criterion_group!(benches, guard_request, guard_event, selector_extract, hermes_request);
criterion_main!(benches);
