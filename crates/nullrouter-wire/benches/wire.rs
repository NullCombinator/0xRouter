//! R24 hot paths (T137): request translation for every bundled style pair, stream
//! translation throughput, usage extraction.
//!
//! `cargo bench -p nullrouter-wire --bench wire -- --save-baseline slice-003`

use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use nullrouter_registry::validate::validate_style;
use nullrouter_wire::codec::{Style, request, response};
use nullrouter_wire::ir::{
    BlockKind, Event, FinishReason, Message, Part, Request, Response, Role, Tool, ToolChoice, Usage,
};
use nullrouter_wire::stream::{Framer, StreamReader, StreamWriter};
use nullrouter_wire::template::Bindings;
use nullrouter_wire::usage;
use serde_json::{Value, json};

const IDS: [&str; 4] = ["anthropic-messages", "openai-chat", "openai-responses", "gemini"];

fn bundled(id: &str) -> Style {
    let path = format!("{}/../../styles/bundled/{id}.toml", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap();
    Style::compile(&validate_style(&src, &path).unwrap()).unwrap()
}

/// An agent turn: system prompt, ten exchanges with a tool call each, two tools.
fn conversation() -> Request {
    let mut req = Request { model: "m1".into(), stream: true, ..Request::default() };
    req.system = vec![Part::text("You are a careful coding agent. ".repeat(40))];
    for i in 0..10 {
        let id = format!("c{i}");
        req.messages.push(Message {
            role: Role::User,
            parts: vec![Part::text(format!("step {i}: {}", "context ".repeat(60)))],
        });
        req.messages.push(Message {
            role: Role::Assistant,
            parts: vec![
                Part::text("reading the file"),
                Part::ToolCall {
                    id: id.clone(),
                    name: "read".into(),
                    arguments: json!({ "path": "src/lib.rs" }),
                    cache_control: None,
                },
            ],
        });
        req.messages.push(Message {
            role: Role::User,
            parts: vec![Part::ToolResult {
                id,
                name: Some("read".into()),
                content: nullrouter_wire::ir::ResultContent::Text("fn main() {}\n".repeat(30)),
                is_error: false,
                cache_control: None,
            }],
        });
    }
    req.tools = ["read", "write"]
        .into_iter()
        .map(|name| Tool {
            name: name.into(),
            description: Some("file access".into()),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
            cache_control: None,
        })
        .collect();
    req.tool_choice = Some(ToolChoice::Auto);
    req.params.values.insert("max_tokens".into(), json!(4096));
    req
}

/// A streamed answer: a thinking block, 200 text deltas, one tool call, usage.
fn answer_events() -> Vec<Event> {
    let mut events = vec![
        Event::Preamble { id: Some("r1".into()), model: Some("m1".into()) },
        Event::BlockStart(BlockKind::Thinking),
        Event::ThinkingDelta("considering the request".into()),
        Event::BlockStop,
        Event::BlockStart(BlockKind::Text),
    ];
    events.extend((0..200).map(|i| Event::TextDelta(format!("token{i} "))));
    events.extend([
        Event::BlockStop,
        Event::BlockStart(BlockKind::ToolCall { id: "c1".into(), name: "write".into() }),
        Event::ToolArguments("{\"path\":".into()),
        Event::ToolArguments("\"src/lib.rs\"}".into()),
        Event::BlockStop,
        Event::Usage(Usage { input: Some(1200), output: Some(240), cache_read: Some(900), ..Usage::default() }),
        Event::Finish(FinishReason::ToolCalls),
        Event::Done,
    ]);
    events
}

fn bench(c: &mut Criterion) {
    let styles: Vec<(&str, Style)> = IDS.iter().map(|id| (*id, bundled(id))).collect();
    let conv = conversation();

    let mut group = c.benchmark_group("request");
    for (a, sa) in &styles {
        let body = request::encode(&conv, sa, "ir").unwrap().body;
        for (b, sb) in &styles {
            group.bench_function(format!("{a}->{b}"), |bench| {
                bench.iter(|| {
                    let req = request::decode(sa, black_box(&body)).unwrap();
                    black_box(request::encode(&req, sb, a).unwrap())
                })
            });
        }
    }
    group.finish();

    let events = answer_events();
    let client_request = json!({ "stream_options": { "include_usage": true } });
    let mut group = c.benchmark_group("stream");
    for (a, sa) in &styles {
        let mut w = StreamWriter::new(sa, &client_request, "r1", "m1", 0).unwrap();
        let mut upstream: String = events.iter().map(|e| w.write(e)).collect();
        upstream.push_str(&w.end());
        group.throughput(Throughput::Bytes(upstream.len() as u64));
        let framing = sa.text.as_ref().unwrap().framing;
        for (b, sb) in &styles {
            group.bench_function(format!("{a}->{b}"), |bench| {
                bench.iter(|| {
                    let mut framer = Framer::new(framing);
                    let mut reader = StreamReader::new(sa).unwrap();
                    let mut writer = StreamWriter::new(sb, &client_request, "r1", "m1", 0).unwrap();
                    let mut out = String::new();
                    for frame in framer.feed(black_box(upstream.as_bytes())).iter().chain(&framer.finish()) {
                        for ev in reader.read(frame).unwrap() {
                            out.push_str(&writer.write(&ev));
                        }
                    }
                    for ev in reader.finish() {
                        out.push_str(&writer.write(&ev));
                    }
                    out.push_str(&writer.end());
                    black_box(out)
                })
            });
        }
    }
    group.finish();

    let answer = Response {
        id: Some("r1".into()),
        model: Some("m1".into()),
        content: vec![Part::text("ok")],
        usage: Some(Usage {
            input: Some(1200),
            output: Some(240),
            cache_read: Some(900),
            cache_write: Some(50),
            reasoning: Some(30),
        }),
        finish: Some(FinishReason::Stop),
    };
    let mut group = c.benchmark_group("usage");
    for (a, sa) in &styles {
        let body: Value = response::encode(sa, &answer, 0).unwrap();
        let sel = &sa.text.as_ref().unwrap().usage;
        group.bench_function(*a, |bench| {
            bench.iter_batched(
                Bindings::default,
                |b| black_box(usage::read(sel, Some(black_box(&body)), &b)),
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
