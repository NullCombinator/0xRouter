//! What every style's guardrail module shares: the compiled style, a conversation that
//! round-trips through all four, and helpers that edit it and ask the guardrail.

use nullrouter_adapters::guard::{self, Verdict};
use nullrouter_adapters::record::GuardrailRule;
use nullrouter_registry::validate::validate_style;
use nullrouter_wire::codec::{Style, request};
use nullrouter_wire::ir::{Message, Part, Request, ResultContent, Role, Tool, ToolChoice};
use serde_json::{Value, json};

pub fn style(id: &str) -> Style {
    let path = format!("{}/../../styles/bundled/{id}.toml", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap();
    let parsed = validate_style(&src, &path).unwrap_or_else(|e| panic!("{id} fails the style gate: {e:#?}"));
    Style::compile(&parsed).unwrap()
}

/// A user turn, an assistant tool call, and the result with a closing remark: the shape the
/// wire crate's own round-trip test proves survives all four styles.
fn conversation() -> Request {
    let mut req = Request { model: "m1".into(), stream: false, ..Request::default() };
    req.system = vec![Part::text("be brief")];
    req.messages = vec![
        Message { role: Role::User, parts: vec![Part::text("weather?")] },
        Message {
            role: Role::Assistant,
            parts: vec![
                Part::text("checking"),
                Part::ToolCall {
                    id: "c1".into(),
                    name: "get_weather".into(),
                    arguments: json!({ "city": "Oslo", "unit": "c" }),
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
    req
}

/// The conversation as a client of `id` sends it, and as the pipeline decodes it.
pub fn original(id: &str) -> (Style, Value, Request) {
    let s = style(id);
    let body = request::encode(&conversation(), &s, id).unwrap_or_else(|e| panic!("encode into {id}: {e}")).body;
    let ir = request::decode(&s, &body).unwrap_or_else(|e| panic!("decode {id}: {e}\n{body:#}"));
    (s, body, ir)
}

/// The guardrail's verdict on the decoded conversation after `edit`, written back out.
pub fn edited(id: &str, edit: impl FnOnce(&mut Request)) -> Verdict {
    let (s, _, before) = original(id);
    let mut after = before.clone();
    edit(&mut after);
    let body = request::encode(&after, &s, id).unwrap_or_else(|e| panic!("encode the edit into {id}: {e}")).body;
    guard::check_request(&s, &before, &body)
}

/// The verdict on the original body after `edit` changed its JSON.
pub fn edited_json(id: &str, edit: impl FnOnce(&mut Value)) -> Verdict {
    let (s, mut body, before) = original(id);
    edit(&mut body);
    guard::check_request(&s, &before, &body)
}

/// Where each style keeps a message's content array, and a block no template of it reads.
fn opaque_slot(id: &str) -> (&'static str, Value) {
    match id {
        "anthropic-messages" => ("/messages/0/content", json!({"type": "zz_unknown_block", "n": 1})),
        "openai-chat" => ("/messages/1/content", json!({"type": "zz_unknown_block", "n": 1})),
        "openai-responses" => ("/input/0/content", json!({"type": "zz_unknown_block", "n": 1})),
        "gemini" => ("/contents/0/parts", json!({"zzUnknownBlock": {"n": 1}})),
        other => panic!("no opaque slot for {other}"),
    }
}

/// The conversation with one opaque block in its first user turn. The block is turned into an
/// array element, so the user turn's content must be an array in `id`'s encoding.
pub fn with_opaque(id: &str) -> (Style, Value, Request) {
    let (s, mut body, _) = original(id);
    let (at, block) = opaque_slot(id);
    let slot = body.pointer_mut(at).unwrap_or_else(|| panic!("{id}: nothing at {at}"));
    if let Some(text) = slot.as_str().map(str::to_owned) {
        *slot = json!([{"type": "text", "text": text}]);
    }
    slot.as_array_mut().unwrap_or_else(|| panic!("{id}: {at} holds no array")).push(block);
    let ir = request::decode(&s, &body).unwrap();
    assert_eq!(ir.opaque.len(), 1, "{id}: the block should decode as opaque\n{body:#}");
    (s, body, ir)
}

pub fn parts(req: &mut Request) -> impl Iterator<Item = &mut Part> {
    req.messages.iter_mut().flat_map(|m| m.parts.iter_mut())
}

/// The first part of `req` for which `is` holds.
pub fn first(req: &mut Request, is: fn(&Part) -> bool) -> &mut Part {
    parts(req).find(|p| is(p)).expect("the conversation has this part")
}

pub fn is_call(p: &Part) -> bool {
    matches!(p, Part::ToolCall { .. })
}

pub fn is_result(p: &Part) -> bool {
    matches!(p, Part::ToolResult { .. })
}

/// Adds `part` to the message that already holds a part for which `like` holds.
pub fn add_beside(req: &mut Request, like: fn(&Part) -> bool, part: Part) {
    let m = req.messages.iter_mut().find(|m| m.parts.iter().any(like)).expect("a message with this part");
    m.parts.push(part);
}

/// Removes every part for which `gone` holds, and any message that is left empty.
pub fn remove(req: &mut Request, gone: fn(&Part) -> bool) {
    for m in &mut req.messages {
        m.parts.retain(|p| !gone(p));
    }
    req.messages.retain(|m| !m.parts.is_empty());
}

pub fn violates(v: &Verdict, rule: GuardrailRule) {
    assert!(matches!(v, Verdict::Violation { rule: r, .. } if *r == rule), "wanted {rule:?}, got {v:?}");
}
