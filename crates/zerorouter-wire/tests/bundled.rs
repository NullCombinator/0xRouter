//! Every bundled style (T049–T052) against every other: a request, a response and a
//! stream written in one style read back the same through the IR.

use serde_json::{Value, json};
use zerorouter_registry::validate::validate_style;
use zerorouter_wire::codec::{Style, request, response};
use zerorouter_wire::ir::{
    BlockKind, Event, FinishReason, Media, MediaSource, Message, Part, Request, Response, ResultContent, Role, Tool,
    ToolChoice, Usage,
};
use zerorouter_wire::stream::{Frame, Framer, StreamReader, StreamWriter};

const IDS: [&str; 4] = ["anthropic-messages", "openai-chat", "openai-responses", "gemini"];

fn bundled(id: &str) -> Style {
    let path = format!("{}/../../styles/bundled/{id}.toml", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap();
    let parsed = validate_style(&src, &path).unwrap_or_else(|e| panic!("{id} fails the style gate: {e:#?}"));
    Style::compile(&parsed).unwrap()
}

fn conversation() -> Request {
    let mut req = Request { model: "m1".into(), stream: false, ..Request::default() };
    req.system = vec![Part::text("be brief")];
    req.messages = vec![
        Message {
            role: Role::User,
            parts: vec![
                Part::text("weather?"),
                Part::Image {
                    media: Media { mime: Some("image/png".into()), source: MediaSource::Base64("AAA".into()) },
                    cache_control: None,
                },
            ],
        },
        Message {
            role: Role::Assistant,
            parts: vec![
                Part::text("checking"),
                Part::ToolCall {
                    id: "c1".into(),
                    name: "get_weather".into(),
                    arguments: json!({ "city": "Oslo" }),
                    cache_control: None,
                },
            ],
        },
        Message {
            role: Role::User,
            parts: vec![
                Part::ToolResult {
                    id: "c1".into(),
                    name: Some("get_weather".into()),
                    content: ResultContent::Text("rain".into()),
                    is_error: false,
                    cache_control: None,
                },
                Part::text("thanks"),
            ],
        },
    ];
    req.tools = vec![Tool {
        name: "get_weather".into(),
        description: Some("d".into()),
        parameters: json!({ "type": "object", "properties": { "city": { "type": "string" } } }),
        cache_control: None,
    }];
    req.tool_choice = Some(ToolChoice::Auto);
    req.params.values.insert("max_tokens".into(), json!(100));
    req.params.values.insert("temperature".into(), json!(0.5));
    req
}

/// What a client sees of a conversation, whatever the layout: each part with the role
/// that sent it (tool results count as the user's), and a tool result's text.
fn seen(req: &Request) -> Vec<(Role, Value)> {
    let mut out: Vec<(Role, Value)> = req.system.iter().map(|p| (Role::System, part(p))).collect();
    for m in &req.messages {
        let role = if m.role == Role::Tool { Role::User } else { m.role };
        out.extend(m.parts.iter().map(|p| (role, part(p))));
    }
    out
}

fn part(p: &Part) -> Value {
    match p {
        Part::Text { text, .. } => json!({ "text": text }),
        Part::Image { media, .. } => json!({ "image": format!("{media:?}") }),
        Part::ToolCall { id, name, arguments, .. } => json!({ "call": [id, name, arguments] }),
        Part::ToolResult { id, content, .. } => json!({ "result": [id, result_text(content)] }),
        other => json!({ "other": format!("{other:?}") }),
    }
}

fn result_text(c: &ResultContent) -> String {
    match c {
        ResultContent::Text(t) => t.clone(),
        ResultContent::Parts(ps) => {
            ps.iter().filter_map(|p| if let Part::Text { text, .. } = p { Some(text.as_str()) } else { None }).collect()
        }
        ResultContent::Json(v) => v
            .get("content")
            .or_else(|| v.get("result"))
            .and_then(Value::as_str)
            .map_or_else(|| v.to_string(), str::to_owned),
    }
}

#[test]
fn a_conversation_survives_every_style_pair() {
    let want = seen(&conversation());
    for a in IDS {
        let sa = bundled(a);
        let body = request::encode(&conversation(), &sa, "ir").unwrap_or_else(|e| panic!("encode into {a}: {e}")).body;
        let read = request::decode(&sa, &body).unwrap_or_else(|e| panic!("decode {a}: {e}\n{body:#}"));
        assert!(
            read.opaque.is_empty() && read.unplaced.is_empty(),
            "{a}: {:?} {:?}\n{body:#}",
            read.opaque,
            read.unplaced
        );
        assert_eq!(seen(&read), want, "{a}\n{body:#}");
        assert_eq!(read.tools.len(), 1, "{a}");
        assert_eq!(read.tool_choice, Some(ToolChoice::Auto), "{a}");
        assert_eq!(read.params.values.get("max_tokens"), Some(&json!(100)), "{a}");
        // In its own style the body goes back out unchanged.
        assert_eq!(request::encode(&read, &sa, a).unwrap().body, body, "{a}");
        for b in IDS {
            let sb = bundled(b);
            let across = request::encode(&read, &sb, a).unwrap_or_else(|e| panic!("{a} → {b}: {e}")).body;
            let back = request::decode(&sb, &across).unwrap_or_else(|e| panic!("{a} → {b} decode: {e}\n{across:#}"));
            assert_eq!(seen(&back), want, "{a} → {b}\n{across:#}");
        }
    }
}

fn answer() -> Response {
    Response {
        id: Some("r1".into()),
        model: Some("m1".into()),
        content: vec![
            Part::text("ok"),
            Part::ToolCall { id: "c1".into(), name: "f".into(), arguments: json!({ "a": 1 }), cache_control: None },
        ],
        usage: Some(Usage { input: Some(10), output: Some(2), cache_read: Some(4), ..Usage::default() }),
        finish: Some(FinishReason::ToolCalls),
    }
}

#[test]
fn a_response_survives_every_style() {
    for a in IDS {
        let s = bundled(a);
        let body = response::encode(&s, &answer(), 7).unwrap_or_else(|e| panic!("encode {a}: {e}"));
        let r = response::decode(&s, &body).unwrap_or_else(|e| panic!("decode {a}: {e}\n{body:#}"));
        assert_eq!(r.content, answer().content, "{a}\n{body:#}");
        assert_eq!(r.finish, Some(FinishReason::ToolCalls), "{a}\n{body:#}");
        let u = r.usage.unwrap();
        assert_eq!((u.input, u.output, u.cache_read), (Some(10), Some(2), Some(4)), "{a}\n{body:#}");
    }
}

fn read_stream(style: &Style, bytes: &str) -> Vec<Event> {
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
fn a_stream_survives_every_style() {
    let events = [
        Event::Preamble { id: Some("r1".into()), model: Some("m1".into()) },
        Event::BlockStart(BlockKind::Thinking),
        Event::ThinkingDelta("hmm".into()),
        Event::BlockStop,
        Event::BlockStart(BlockKind::Text),
        Event::TextDelta("Hel".into()),
        Event::TextDelta("lo".into()),
        Event::BlockStop,
        Event::BlockStart(BlockKind::ToolCall { id: "c1".into(), name: "f".into() }),
        Event::ToolArguments("{\"a\":".into()),
        Event::ToolArguments("1}".into()),
        Event::BlockStop,
        Event::Usage(Usage { input: Some(10), output: Some(4), ..Usage::default() }),
        Event::Finish(FinishReason::ToolCalls),
        Event::Done,
    ];
    let request = json!({ "stream_options": { "include_usage": true } });
    for a in IDS {
        let s = bundled(a);
        let mut w = StreamWriter::new(&s, &request, "r1", "m1", 0).unwrap();
        let mut bytes: String = events.iter().map(|e| w.write(e)).collect();
        bytes.push_str(&w.end());
        let back = read_stream(&s, &bytes);
        let joined = |f: fn(&Event) -> Option<&str>| back.iter().filter_map(f).collect::<String>();
        assert_eq!(joined(|e| if let Event::TextDelta(t) = e { Some(t) } else { None }), "Hello", "{a}\n{bytes}");
        assert_eq!(joined(|e| if let Event::ThinkingDelta(t) = e { Some(t) } else { None }), "hmm", "{a}\n{bytes}");
        let args: Value =
            serde_json::from_str(&joined(|e| if let Event::ToolArguments(t) = e { Some(t) } else { None }))
                .unwrap_or_else(|e| panic!("{a}: arguments: {e}\n{bytes}"));
        assert_eq!(args, json!({ "a": 1 }), "{a}");
        assert!(
            back.iter()
                .any(|e| matches!(e, Event::BlockStart(BlockKind::ToolCall { id, name }) if id == "c1" && name == "f")),
            "{a}: {back:#?}"
        );
        let finish = back.iter().rev().find_map(|e| if let Event::Finish(f) = e { Some(*f) } else { None });
        assert_eq!(finish, Some(FinishReason::ToolCalls), "{a}\n{bytes}");
        let input = back.iter().rev().find_map(|e| if let Event::Usage(u) = e { u.input } else { None });
        assert_eq!(input, Some(10), "{a}: {back:#?}\n{bytes}");
    }
}
