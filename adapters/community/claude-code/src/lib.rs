//! Claude Code adapter (research R15).
//!
//! Ports 9router's `normalizeClaudePassthrough` (`open-sse/translator/formats/claude.js:204`)
//! and `dedupeTools` (`open-sse/utils/toolDeduper.js:33`) as edits on the client's
//! anthropic-messages body:
//!
//! 1. adaptive thinking on a haiku model becomes `enabled` with a 10 000-token budget;
//! 2. `output_config.effort` on a haiku model is removed, with an `output_config` left empty;
//! 3. a bare content-block object is wrapped as a one-element array, without its
//!    `cache_control`;
//! 4. a mid-conversation `role: "system"` message is folded into the user turn before it, or
//!    becomes a user turn of its own;
//! 5. on Anthropic, same style: thinking blocks without a Claude signature are removed, and so
//!    are `server_tool_use` blocks with a foreign id and the results that reference them;
//! 6. on any other target: `server_tool_use` and `web_search_tool_result` blocks are removed;
//! 7. empty text blocks, and messages left empty, are removed;
//! 8. built-in tools with an MCP equivalent are removed.
//!
//! Unlike 9router, no thinking placeholder is inserted into a tool-use turn: an adapter only
//! removes or converts.

use std::borrow::Cow;
use std::collections::BTreeSet;

use nullrouter_adapter_kit::serde_json::{Map, Value, json};
use nullrouter_adapter_kit::{Adapter, Context, Edits, Input, Path, Reason, Seg};

/// The Claude Code adapter.
pub struct ClaudeCode;

impl Adapter for ClaudeCode {
    fn on_request(ctx: &Context, input: &Input, out: &mut Edits) {
        let parts = Parts::read(input);
        // 9router tests the upstream model (`translatedBody.model`) against `/haiku/i`.
        let haiku = ctx.model.to_ascii_lowercase().contains("haiku");
        let anthropic = ctx.provider == "anthropic" && ctx.same_style;
        downgrade_thinking(haiku, parts.thinking, out);
        strip_effort(haiku, parts.output_config, out);
        normalize_messages(anthropic, &parts.messages, out);
        dedupe_tools(parts.tools, out);
    }
}

nullrouter_adapter_kit::export!(ClaudeCode);

/// The selected parts of the body.
#[derive(Default)]
struct Parts<'a> {
    thinking: Option<&'a Value>,
    output_config: Option<&'a Value>,
    tools: Option<&'a Value>,
    /// `messages[i]`, in index order.
    messages: Vec<(usize, &'a Value)>,
}

impl<'a> Parts<'a> {
    fn read(input: &'a Input) -> Parts<'a> {
        let mut parts = Parts::default();
        for (path, value) in input.parts() {
            match path.0.as_slice() {
                [Seg::Key(k)] if k == "thinking" => parts.thinking = Some(value),
                [Seg::Key(k)] if k == "output_config" => parts.output_config = Some(value),
                [Seg::Key(k)] if k == "tools" => parts.tools = Some(value),
                [Seg::Key(k), Seg::Index(i)] if k == "messages" => parts.messages.push((*i, value)),
                _ => {}
            }
        }
        parts.messages.sort_by_key(|(i, _)| *i);
        parts
    }
}

fn top(key: &str) -> Path {
    Path::root().child(key)
}

fn message(i: usize) -> Path {
    top("messages").index(i)
}

// ---------------------------------------------------------------------------------------------
// Steps 1 and 2: haiku parameters

fn downgrade_thinking(haiku: bool, thinking: Option<&Value>, out: &mut Edits) {
    let adaptive = thinking.and_then(|t| t.get("type")).and_then(Value::as_str) == Some("adaptive");
    if haiku && adaptive {
        let enabled = json!({"type": "enabled", "budget_tokens": 10000});
        out.convert(&top("thinking"), enabled, Reason::ParamUnsupportedByModel);
    }
}

fn strip_effort(haiku: bool, output_config: Option<&Value>, out: &mut Edits) {
    let Some(Value::Object(config)) = output_config else { return };
    if !haiku || config.get("effort").is_none_or(Value::is_null) {
        return;
    }
    let path = if config.len() == 1 { top("output_config") } else { top("output_config").child("effort") };
    out.remove(&path, Reason::ParamUnsupportedByModel);
}

// ---------------------------------------------------------------------------------------------
// Steps 3 to 7: messages

/// A content block as 9router's passes see it, with its index in the client's array if it had
/// one there.
struct Block<'a> {
    at: Option<usize>,
    value: Cow<'a, Value>,
}

enum Content<'a> {
    Text(&'a str),
    Blocks(Vec<Block<'a>>),
    /// Missing, `null`, or any other shape 9router leaves alone.
    Other,
}

/// How a turn's final form is written back as edits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The client's shape: each removed block is its own edit.
    Kept,
    /// A bare block wrapped as an array: `content` is replaced.
    Wrapped,
    /// System text folded in: `content` is replaced.
    Folded,
    /// A system message turned into a user message: the message is replaced.
    FromSystem,
}

struct Turn<'a> {
    /// Index of the message in the client's body.
    at: usize,
    /// `None` when the message isn't an object.
    msg: Option<&'a Map<String, Value>>,
    role: Option<&'a str>,
    content: Content<'a>,
    shape: Shape,
    /// Removed blocks the client sent in an array: their index and why.
    dropped: Vec<(usize, Reason)>,
    emptied: bool,
}

impl<'a> Turn<'a> {
    /// Step 3 happens here: a bare content-block object is wrapped, and its `cache_control`
    /// deleted.
    fn new(at: usize, value: &'a Value) -> Turn<'a> {
        let msg = value.as_object();
        let role = msg.and_then(|m| m.get("role")).and_then(Value::as_str);
        let (content, shape) = match msg.and_then(|m| m.get("content")) {
            Some(Value::String(s)) => (Content::Text(s), Shape::Kept),
            Some(Value::Array(blocks)) => {
                let blocks = blocks.iter().enumerate().map(|(j, b)| Block { at: Some(j), value: Cow::Borrowed(b) });
                (Content::Blocks(blocks.collect()), Shape::Kept)
            }
            Some(Value::Object(block)) => {
                let block: Map<String, Value> =
                    block.iter().filter(|(k, _)| *k != "cache_control").map(|(k, v)| (k.clone(), v.clone())).collect();
                let wrapped = Block { at: None, value: Cow::Owned(Value::Object(block)) };
                (Content::Blocks(vec![wrapped]), Shape::Wrapped)
            }
            _ => (Content::Other, Shape::Kept),
        };
        Turn { at, msg, role, content, shape, dropped: Vec::new(), emptied: false }
    }

    /// Removes the blocks `pick` names a reason for.
    fn drop_blocks(&mut self, mut pick: impl FnMut(&Value) -> Option<Reason>) {
        let Content::Blocks(blocks) = &mut self.content else { return };
        let dropped = &mut self.dropped;
        blocks.retain(|b| match pick(&*b.value) {
            Some(reason) => {
                if let Some(j) = b.at {
                    dropped.push((j, reason));
                }
                false
            }
            None => true,
        });
    }

    fn content_value(&self) -> Value {
        match &self.content {
            Content::Text(s) => Value::String((*s).to_owned()),
            Content::Blocks(blocks) => Value::Array(blocks.iter().map(|b| Value::clone(&b.value)).collect()),
            Content::Other => Value::Null,
        }
    }
}

fn text_block(text: &str) -> Block<'static> {
    Block { at: None, value: Cow::Owned(json!({"type": "text", "text": text})) }
}

fn block_type(block: &Value) -> Option<&str> {
    block.get("type").and_then(Value::as_str)
}

fn normalize_messages(anthropic: bool, messages: &[(usize, &Value)], out: &mut Edits) {
    let (mut turns, folded) = fold_system(messages.iter().map(|(i, v)| Turn::new(*i, *v)));

    if anthropic {
        drop_foreign(&mut turns);
    } else {
        for turn in &mut turns {
            turn.drop_blocks(|b| {
                matches!(block_type(b), Some("server_tool_use" | "web_search_tool_result"))
                    .then_some(Reason::TargetCannotCarryBlock)
            });
        }
    }
    drop_empty(&mut turns);

    for at in folded {
        out.remove(&message(at), Reason::RoleNotAccepted);
    }
    for turn in &turns {
        write_back(turn, out);
    }
}

/// Step 4. Returns the turns that stay and the indices of system messages folded away or
/// dropped as blank.
fn fold_system<'a>(turns: impl Iterator<Item = Turn<'a>>) -> (Vec<Turn<'a>>, Vec<usize>) {
    let mut kept: Vec<Turn<'a>> = Vec::new();
    let mut folded = Vec::new();
    for turn in turns {
        if turn.role != Some("system") {
            kept.push(turn);
            continue;
        }
        let text = match &turn.content {
            Content::Text(s) => (*s).to_owned(),
            Content::Blocks(blocks) => blocks.iter().map(|b| system_piece(&b.value)).collect::<Vec<_>>().join("\n"),
            Content::Other => String::new(),
        };
        if js_trim(&text).is_empty() {
            folded.push(turn.at);
            continue;
        }
        if let Some(prev) = kept.last_mut().filter(|p| p.role == Some("user")) {
            let mut blocks = match std::mem::replace(&mut prev.content, Content::Other) {
                Content::Text(s) => vec![text_block(s)],
                Content::Blocks(blocks) => blocks,
                Content::Other => Vec::new(),
            };
            blocks.push(text_block(&text));
            prev.content = Content::Blocks(blocks);
            if prev.shape != Shape::FromSystem {
                prev.shape = Shape::Folded;
            }
            folded.push(turn.at);
            continue;
        }
        kept.push(Turn {
            at: turn.at,
            msg: turn.msg,
            role: Some("user"),
            content: Content::Blocks(vec![text_block(&text)]),
            shape: Shape::FromSystem,
            dropped: Vec::new(),
            emptied: false,
        });
    }
    (kept, folded)
}

/// `typeof b === "string" ? b : b?.text || ""`.
fn system_piece(block: &Value) -> String {
    match block {
        Value::String(s) => s.clone(),
        other => other.get("text").filter(|t| js_truthy(t)).map(js_string).unwrap_or_default(),
    }
}

/// Step 5: Anthropic, same style.
fn drop_foreign(turns: &mut [Turn<'_>]) {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for turn in turns.iter_mut().filter(|t| t.role == Some("assistant")) {
        turn.drop_blocks(|b| match block_type(b) {
            Some("thinking" | "redacted_thinking") => {
                (!is_valid_claude_signature(b.get("signature"))).then_some(Reason::ForeignBlock)
            }
            Some("server_tool_use") if !is_native_server_tool_id(&js_string_or_empty(b.get("id"))) => {
                if let Some(id) = b.get("id").filter(|id| !id.is_null()) {
                    ids.insert(js_string(id));
                }
                Some(Reason::ForeignBlock)
            }
            _ => None,
        });
    }
    if ids.is_empty() {
        return;
    }
    // A result whose server_tool_use is gone would reference an id no block declares.
    for turn in turns.iter_mut() {
        turn.drop_blocks(|b| {
            let result = matches!(block_type(b), Some("tool_result" | "web_search_tool_result"));
            (result && ids.contains(&js_string_or_empty(b.get("tool_use_id")))).then_some(Reason::ForeignBlock)
        });
    }
}

/// Step 7.
fn drop_empty(turns: &mut [Turn<'_>]) {
    for turn in turns {
        turn.drop_blocks(|b| {
            let blank = block_type(b) == Some("text") && js_trim(&js_string_or_empty(b.get("text"))).is_empty();
            blank.then_some(Reason::EmptyAfterRemoval)
        });
        turn.emptied = match &turn.content {
            Content::Text(s) => js_trim(s).is_empty(),
            Content::Blocks(blocks) => blocks.is_empty(),
            Content::Other => false,
        };
    }
}

fn write_back(turn: &Turn<'_>, out: &mut Edits) {
    let path = message(turn.at);
    if turn.emptied {
        out.remove(&path, Reason::EmptyAfterRemoval);
        return;
    }
    let reason = match turn.shape {
        Shape::Kept => {
            for (j, reason) in &turn.dropped {
                out.remove(&path.child("content").index(*j), *reason);
            }
            return;
        }
        Shape::FromSystem => {
            let user = json!({"role": "user", "content": turn.content_value()});
            out.convert(&path, user, Reason::RoleNotAccepted);
            return;
        }
        Shape::Wrapped => Reason::FormatConversion,
        Shape::Folded => Reason::RoleNotAccepted,
    };
    let Some(msg) = turn.msg else { return };
    if msg.contains_key("content") {
        out.convert(&path.child("content"), turn.content_value(), reason);
    } else {
        let mut whole = msg.clone();
        whole.insert("content".to_owned(), turn.content_value());
        out.convert(&path, Value::Object(whole), reason);
    }
}

/// `CLAUDE_SERVER_TOOL_USE_ID = /^srvtoolu_[a-zA-Z0-9_]+$/` (`claude.js:190`).
fn is_native_server_tool_id(id: &str) -> bool {
    id.strip_prefix("srvtoolu_")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

// ---------------------------------------------------------------------------------------------
// Claude signature check, ported from `open-sse/utils/claudeSignature.js:9-41`

const MAX_CLAUDE_SIGNATURE_LEN: usize = 32 * 1024 * 1024;
const CLAUDE_SIGNATURE_MARKER: u8 = 0x12;

/// `stripCachePrefix`: trims, then keeps what follows the first `#`, trimmed.
fn strip_cache_prefix(raw: &str) -> &str {
    let sig = js_trim(raw);
    match sig.find('#') {
        Some(i) => js_trim(&sig[i + 1..]),
        None => sig,
    }
}

/// `isValidClaudeSignature`. E-form: one base64 layer whose first byte is `0x12`. R-form: two
/// layers, the outer starting with `E`, the inner with `0x12`. A missing or non-string
/// signature is invalid (9router throws on a truthy non-string one).
fn is_valid_claude_signature(raw: Option<&Value>) -> bool {
    let Some(Value::String(raw)) = raw else { return false };
    let sig = strip_cache_prefix(raw);
    if sig.is_empty() || (sig.len() > MAX_CLAUDE_SIGNATURE_LEN && sig.encode_utf16().count() > MAX_CLAUDE_SIGNATURE_LEN) {
        return false;
    }
    if sig.starts_with('E') {
        return node_base64(sig).first() == Some(&CLAUDE_SIGNATURE_MARKER);
    }
    if sig.starts_with('R') {
        let outer = node_base64(sig);
        if outer.first() != Some(&b'E') {
            return false;
        }
        let inner = node_base64(&String::from_utf8_lossy(&outer));
        return inner.first() == Some(&CLAUDE_SIGNATURE_MARKER);
    }
    false
}

fn sextet(c: u8) -> Option<u32> {
    let v = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' | b'-' => 62,
        b'/' | b'_' => 63,
        _ => return None,
    };
    Some(u32::from(v))
}

/// Node's `Buffer.from(s, "base64")`: each UTF-16 code unit is read by its low byte, both
/// alphabets are accepted, other characters are skipped, and decoding stops at the first `=`.
fn node_base64(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut n) = (0u32, 0u8);
    for unit in s.encode_utf16() {
        let c = unit.to_le_bytes()[0];
        if c == b'=' {
            break;
        }
        let Some(v) = sextet(c) else { continue };
        acc = (acc << 6) | v;
        n += 1;
        if n == 4 {
            out.extend_from_slice(&acc.to_be_bytes()[1..]);
            (acc, n) = (0, 0);
        }
    }
    match n {
        2 => out.push((acc >> 4).to_be_bytes()[3]),
        3 => out.extend_from_slice(&(acc >> 2).to_be_bytes()[2..]),
        _ => {}
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Step 8: duplicate tools, ported from `open-sse/utils/toolDeduper.js:6-47`

enum Pattern {
    Name(&'static str),
    Prefix(&'static str),
}

impl Pattern {
    fn matches(&self, name: &str) -> bool {
        match self {
            Pattern::Name(n) => name == *n,
            Pattern::Prefix(p) => name.starts_with(p),
        }
    }
}

const WEB_BUILTINS: [Pattern; 3] =
    [Pattern::Name("WebSearch"), Pattern::Name("WebFetch"), Pattern::Name("mcp__workspace__web_fetch")];

/// `(triggers, strip)`: when any tool matches a trigger, every tool matching `strip` goes.
const DEDUP_RULES: [(&[Pattern], &[Pattern]); 3] = [
    (&[Pattern::Name("mcp__exa__web_search_exa"), Pattern::Name("mcp__exa__web_fetch_exa")], &WEB_BUILTINS),
    (&[Pattern::Name("mcp__tavily__tavily_search"), Pattern::Name("mcp__tavily__tavily_extract")], &WEB_BUILTINS),
    (&[Pattern::Prefix("mcp__browsermcp__")], &[Pattern::Prefix("mcp__Claude_in_Chrome__")]),
];

/// `t?.name || t?.function?.name || ""`.
fn tool_name(tool: &Value) -> &str {
    [tool.get("name"), tool.get("function").and_then(|f| f.get("name"))]
        .into_iter()
        .flatten()
        .find(|v| js_truthy(v))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn dedupe_tools(tools: Option<&Value>, out: &mut Edits) {
    let Some(Value::Array(tools)) = tools else { return };
    let names: Vec<&str> = tools.iter().map(tool_name).collect();
    let strip: Vec<&[Pattern]> = DEDUP_RULES
        .iter()
        .filter(|(triggers, _)| names.iter().any(|n| triggers.iter().any(|p| p.matches(n))))
        .map(|(_, strip)| *strip)
        .collect();
    for (i, name) in names.iter().enumerate() {
        if strip.iter().any(|s| s.iter().any(|p| p.matches(name))) {
            out.remove(&top("tools").index(i), Reason::DuplicateTool);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// JavaScript value semantics the ported code relies on

/// `String.prototype.trim`: Unicode white space and line terminators, with U+FEFF and without
/// U+0085.
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}

fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `String(v)`.
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(items) => {
            items.iter().map(|x| if x.is_null() { String::new() } else { js_string(x) }).collect::<Vec<_>>().join(",")
        }
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// `String(v ?? "")`.
fn js_string_or_empty(v: Option<&Value>) -> String {
    v.filter(|v| !v.is_null()).map(js_string).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nullrouter_adapter_kit::{Capabilities, Direction, Edit, Kind, Op, Part};

    const SONNET: &str = "claude-sonnet-4-5-20250929";
    const HAIKU: &str = "claude-haiku-4-5-20251001";

    fn ctx(provider: &str, style: &str, same_style: bool, model: &str) -> Context {
        Context {
            direction: Direction::Request,
            provider: provider.into(),
            target_style: style.into(),
            same_style,
            model: model.into(),
            model_type: "text".into(),
            capabilities: Capabilities::default(),
            stream: true,
            attempt: 1,
        }
    }

    fn anthropic(model: &str) -> Context {
        ctx("anthropic", "anthropic-messages", true, model)
    }

    fn openrouter() -> Context {
        ctx("openrouter", "openai-chat", false, SONNET)
    }

    /// The parts the host extracts for this adapter's selectors.
    fn parts(body: &Value) -> Input {
        let mut parts = Vec::new();
        for key in ["thinking", "output_config", "model"] {
            if let Some(v) = body.get(key) {
                parts.push(Part { path: top(key), value: v.clone() });
            }
        }
        if let Some(Value::Array(messages)) = body.get("messages") {
            for (i, m) in messages.iter().enumerate() {
                parts.push(Part { path: message(i), value: m.clone() });
            }
        }
        if let Some(v) = body.get("tools") {
            parts.push(Part { path: top("tools"), value: v.clone() });
        }
        Input { parts }
    }

    fn slot<'v>(body: &'v mut Value, path: &Path) -> &'v mut Value {
        let mut cur = body;
        for s in &path.0 {
            cur = match (s, cur) {
                (Seg::Key(k), Value::Object(m)) => m.get_mut(k).expect("key"),
                (Seg::Index(i), Value::Array(a)) => a.get_mut(*i).expect("index"),
                _ => panic!("no such path: {path}"),
            };
        }
        cur
    }

    /// The core's edit checks that need no selector (kind, existence, overlap), then its apply:
    /// replacements first, then removals from the highest path down.
    fn apply(body: &Value, edits: &Edits) -> Value {
        let mut out = body.clone();
        for e in &edits.edits {
            match e.op {
                Op::Remove => assert!(e.kind == Kind::Removed && e.value.is_none(), "{e:?}"),
                Op::Replace => assert!(e.kind == Kind::Converted && e.value.is_some(), "{e:?}"),
            }
            slot(&mut out, &e.path);
        }
        let mut paths: Vec<&Path> = edits.edits.iter().map(|e| &e.path).collect();
        paths.sort();
        assert!(!paths.windows(2).any(|w| w[0].is_prefix_of(w[1])), "overlapping edits: {paths:?}");
        for e in edits.edits.iter().filter(|e| e.op == Op::Replace) {
            *slot(&mut out, &e.path) = e.value.clone().expect("value");
        }
        let mut removals: Vec<&Edit> = edits.edits.iter().filter(|e| e.op == Op::Remove).collect();
        removals.sort_by(|a, b| b.path.cmp(&a.path));
        for e in removals {
            let (last, parent) = e.path.0.split_last().expect("not root");
            match (last, slot(&mut out, &Path(parent.to_vec()))) {
                (Seg::Key(k), Value::Object(m)) => m.retain(|key, _| key != k),
                (Seg::Index(i), Value::Array(a)) => {
                    a.remove(*i);
                }
                _ => panic!("bad removal {}", e.path),
            }
        }
        out
    }

    fn run(ctx: &Context, body: &Value) -> Edits {
        let mut out = Edits::default();
        ClaudeCode::on_request(ctx, &parts(body), &mut out);
        out
    }

    fn reasons(edits: &Edits) -> Vec<(String, Reason)> {
        edits.edits.iter().map(|e| (e.path.to_string(), e.reason)).collect()
    }

    fn base(model: &str, extra: Value) -> Value {
        let mut body = json!({"model": model, "max_tokens": 32000, "stream": true});
        if let (Value::Object(b), Value::Object(e)) = (&mut body, extra) {
            b.extend(e);
        }
        body
    }

    // --- Oracle cases from tests/fixtures/9router/adapters/claude-code (ref 39e36d3) -----------

    #[test]
    fn adaptive_thinking_on_haiku_becomes_enabled() {
        let msgs = json!([{"role": "user", "content": [{"type": "text", "text": "List the files in this directory."}]}]);
        let input = base(HAIKU, json!({"thinking": {"type": "adaptive"}, "messages": msgs}));
        let want = base(HAIKU, json!({"thinking": {"type": "enabled", "budget_tokens": 10000}, "messages": msgs}));
        let edits = run(&anthropic(HAIKU), &input);
        assert_eq!(apply(&input, &edits), want);
        assert_eq!(reasons(&edits), vec![("thinking".into(), Reason::ParamUnsupportedByModel)]);
        assert_eq!(edits.edits[0].kind, Kind::Converted);
        // Not haiku: untouched. The context's model decides, not the body's.
        assert!(run(&anthropic(SONNET), &input).is_empty());
        assert!(!run(&anthropic("CLAUDE-HAIKU-x"), &base(SONNET, json!({"thinking": {"type": "adaptive"}}))).is_empty());
    }

    #[test]
    fn effort_on_haiku_is_removed_with_an_empty_output_config() {
        let msgs = json!([{"role": "user", "content": [{"type": "text", "text": "Summarise README.md."}]}]);
        let input = base(HAIKU, json!({"output_config": {"effort": "high"}, "messages": msgs}));
        let want = base(HAIKU, json!({"messages": msgs}));
        let edits = run(&anthropic(HAIKU), &input);
        assert_eq!(apply(&input, &edits), want);
        assert_eq!(reasons(&edits), vec![("output_config".into(), Reason::ParamUnsupportedByModel)]);

        let other = base(HAIKU, json!({"output_config": {"effort": "high", "format": "x"}}));
        let edits = run(&anthropic(HAIKU), &other);
        assert_eq!(apply(&other, &edits), base(HAIKU, json!({"output_config": {"format": "x"}})));
        // `effort: null` is `== null` in 9router: kept. Not haiku: kept.
        assert!(run(&anthropic(HAIKU), &base(HAIKU, json!({"output_config": {"effort": null}}))).is_empty());
        assert!(run(&anthropic(SONNET), &other).is_empty());
    }

    #[test]
    fn a_bare_content_block_is_wrapped_without_its_cache_control() {
        let input = base(
            SONNET,
            json!({"messages": [
                {"role": "user", "content": {"type": "text", "text": "What does main.rs do?", "cache_control": {"type": "ephemeral"}}},
                {"role": "assistant", "content": {"type": "text", "text": "It starts the server."}},
                {"role": "user", "content": [{"type": "text", "text": "And lib.rs?"}]}]}),
        );
        let want = base(
            SONNET,
            json!({"messages": [
                {"role": "user", "content": [{"type": "text", "text": "What does main.rs do?"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "It starts the server."}]},
                {"role": "user", "content": [{"type": "text", "text": "And lib.rs?"}]}]}),
        );
        let edits = run(&anthropic(SONNET), &input);
        assert_eq!(apply(&input, &edits), want);
        assert!(edits.edits.iter().all(|e| e.reason == Reason::FormatConversion && e.kind == Kind::Converted));
    }

    #[test]
    fn mid_conversation_system_messages_fold_into_the_user_turn_before() {
        let system = json!([{"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude."}]);
        let input = base(
            SONNET,
            json!({"system": system, "messages": [
                {"role": "user", "content": "Fix the failing test."},
                {"role": "system", "content": "<system-reminder>Token usage: 1200/200000</system-reminder>"},
                {"role": "assistant", "content": [{"type": "text", "text": "Looking at the test now."}]},
                {"role": "system", "content": [{"type": "text", "text": "<system-reminder>The user changed lib.rs.</system-reminder>"}]},
                {"role": "system", "content": "   "},
                {"role": "user", "content": [{"type": "text", "text": "Go on."}]}]}),
        );
        let want = base(
            SONNET,
            json!({"system": system, "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Fix the failing test."},
                                             {"type": "text", "text": "<system-reminder>Token usage: 1200/200000</system-reminder>"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Looking at the test now."}]},
                {"role": "user", "content": [{"type": "text", "text": "<system-reminder>The user changed lib.rs.</system-reminder>"}]},
                {"role": "user", "content": [{"type": "text", "text": "Go on."}]}]}),
        );
        let edits = run(&anthropic(SONNET), &input);
        assert_eq!(apply(&input, &edits), want);
        assert!(edits.edits.iter().all(|e| e.reason == Reason::RoleNotAccepted));
    }

    #[test]
    fn consecutive_system_messages_fold_into_one_turn() {
        let input = json!({"messages": [
            {"role": "assistant", "content": "a"},
            {"role": "system", "content": ["one", {"text": "two"}, {"type": "image"}]},
            {"role": "system", "content": "three"}]});
        let want = json!({"messages": [
            {"role": "assistant", "content": "a"},
            {"role": "user", "content": [{"type": "text", "text": "one\ntwo\n"}, {"type": "text", "text": "three"}]}]});
        assert_eq!(apply(&input, &run(&anthropic(SONNET), &input)), want);
    }

    #[test]
    fn foreign_thinking_signatures_are_removed_on_anthropic() {
        let input = base(
            SONNET,
            json!({"thinking": {"type": "enabled", "budget_tokens": 8000}, "messages": [
                {"role": "user", "content": "What is 17 * 23?"},
                {"role": "assistant", "content": [{"type": "thinking", "thinking": "17 * 23 = 391.", "signature": "CiQBKzxNBQYHCAkKCwwNDg8="},
                                                  {"type": "text", "text": "391"}]},
                {"role": "user", "content": "And 391 / 17?"},
                {"role": "assistant", "content": [{"type": "thinking", "thinking": "391 / 17 = 23.", "signature": "EkAKCAgIGAIqQAECAwQFBgcICQoLDA0ODxAREhMUFRYXGA=="},
                                                  {"type": "text", "text": "23"}]},
                {"role": "user", "content": "Thanks."}]}),
        );
        let edits = run(&anthropic(SONNET), &input);
        assert_eq!(reasons(&edits), vec![("messages[1].content[0]".into(), Reason::ForeignBlock)]);
        let out = apply(&input, &edits);
        assert_eq!(out["messages"][1]["content"], json!([{"type": "text", "text": "391"}]));
        assert_eq!(out["messages"][3], input["messages"][3]);
        // Not Anthropic: thinking is not this adapter's business.
        assert!(run(&openrouter(), &input).is_empty());
    }

    #[test]
    fn redacted_thinking_without_a_claude_signature_is_removed() {
        let data = "EmwKAhgBEgy3va3pzix/LafPsn4aDFIT2Xlxh0L5L8rLVyIwxtE3rAFBa8cr3qpPkNRj2YsvFSkSiUJwnu2qqUgHkzuiRiz3J1AnSsvqtUGmyL2BKhQXXDqT0NqLTRd/s+eOOdSl3bcj8Q==";
        let good = "EkAKCAgIGAIqQAECAwQFBgcICQoLDA0ODxAREhMUFRYXGA==";
        let input = base(
            SONNET,
            json!({"thinking": {"type": "enabled", "budget_tokens": 8000}, "messages": [
                {"role": "user", "content": "Plan the refactor."},
                {"role": "assistant", "content": [{"type": "redacted_thinking", "data": data, "signature": "CiQBKzxNBQYHCAkKCwwNDg8="},
                                                  {"type": "text", "text": "Here is the plan."}]},
                {"role": "user", "content": "Start with step one."},
                {"role": "assistant", "content": [{"type": "redacted_thinking", "data": data, "signature": good},
                                                  {"type": "text", "text": "Step one is done."}]},
                {"role": "user", "content": "Next."},
                {"role": "assistant", "content": [{"type": "redacted_thinking", "data": data},
                                                  {"type": "text", "text": "Step two is done."}]},
                {"role": "user", "content": "Good."}]}),
        );
        let edits = run(&anthropic(SONNET), &input);
        assert_eq!(
            reasons(&edits),
            vec![("messages[1].content[0]".into(), Reason::ForeignBlock), ("messages[5].content[0]".into(), Reason::ForeignBlock)]
        );
        let out = apply(&input, &edits);
        assert_eq!(out["messages"][3], input["messages"][3]);
        assert_eq!(out["messages"][5]["content"], json!([{"type": "text", "text": "Step two is done."}]));
    }

    fn web_search_result() -> Value {
        json!([{"type": "web_search_result", "url": "https://blog.rust-lang.org/", "title": "Rust Blog", "encrypted_content": "abc123", "page_age": null}])
    }

    fn server_tool_body(id: &str) -> Value {
        base(
            SONNET,
            json!({"tools": [{"type": "web_search_20250305", "name": "web_search", "max_uses": 5}], "messages": [
                {"role": "user", "content": "What changed in Rust 1.90?"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "Let me search."},
                    {"type": "server_tool_use", "id": id, "name": "web_search", "input": {"query": "Rust 1.90 release notes"}},
                    {"type": "web_search_tool_result", "tool_use_id": id, "content": web_search_result()},
                    {"type": "text", "text": "Rust 1.90 ships lld by default on x86_64 Linux."}]},
                {"role": "user", "content": "Thanks."}]}),
        )
    }

    #[test]
    fn foreign_server_tool_ids_go_with_their_results_on_anthropic() {
        let mut input = server_tool_body("call_9f2c41d7");
        if let Some(Value::Array(msgs)) = input.get_mut("messages") {
            msgs.pop();
            msgs.push(json!({"role": "user", "content": "Search for 1.91 too."}));
            msgs.push(json!({"role": "assistant", "content": [
                {"type": "server_tool_use", "id": "call_a07be3", "name": "web_search", "input": {"query": "Rust 1.91 release notes"}}]}));
            msgs.push(json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "call_a07be3", "content": "no results"},
                {"type": "text", "text": "Anything?"}]}));
        }
        let want = base(
            SONNET,
            json!({"tools": [{"type": "web_search_20250305", "name": "web_search", "max_uses": 5}], "messages": [
                {"role": "user", "content": "What changed in Rust 1.90?"},
                {"role": "assistant", "content": [{"type": "text", "text": "Let me search."},
                                                  {"type": "text", "text": "Rust 1.90 ships lld by default on x86_64 Linux."}]},
                {"role": "user", "content": "Search for 1.91 too."},
                {"role": "user", "content": [{"type": "text", "text": "Anything?"}]}]}),
        );
        let edits = run(&anthropic(SONNET), &input);
        assert_eq!(apply(&input, &edits), want);
        assert_eq!(
            reasons(&edits),
            vec![
                ("messages[1].content[1]".into(), Reason::ForeignBlock),
                ("messages[1].content[2]".into(), Reason::ForeignBlock),
                ("messages[3]".into(), Reason::EmptyAfterRemoval),
                ("messages[4].content[0]".into(), Reason::ForeignBlock),
            ]
        );
        assert!(edits.edits.iter().all(|e| e.op == Op::Remove));
    }

    #[test]
    fn native_server_tool_blocks_stay_on_anthropic() {
        let input = server_tool_body("srvtoolu_01WYG3ziw53XMcoyKL4XcZmE");
        assert!(run(&anthropic(SONNET), &input).is_empty());
    }

    #[test]
    fn server_tool_blocks_are_removed_for_other_targets() {
        let input = server_tool_body("srvtoolu_01WYG3ziw53XMcoyKL4XcZmE");
        for target in [openrouter(), ctx("anthropic", "anthropic-messages", false, SONNET), ctx("deepseek", "anthropic-messages", true, SONNET)] {
            let edits = run(&target, &input);
            assert_eq!(
                reasons(&edits),
                vec![
                    ("messages[1].content[1]".into(), Reason::TargetCannotCarryBlock),
                    ("messages[1].content[2]".into(), Reason::TargetCannotCarryBlock),
                ]
            );
            let out = apply(&input, &edits);
            assert_eq!(out["messages"][1]["content"].as_array().map(Vec::len), Some(2));
        }
        // A turn holding only server-tool blocks is then left empty and removed.
        let only = json!({"messages": [{"role": "user", "content": "q"}, {"role": "assistant", "content": [
            {"type": "server_tool_use", "id": "srvtoolu_x", "name": "web_search", "input": {}},
            {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_x", "content": []}]}]});
        assert_eq!(reasons(&run(&openrouter(), &only)), vec![("messages[1]".into(), Reason::EmptyAfterRemoval)]);
    }

    #[test]
    fn no_thinking_placeholder_is_inserted() {
        let input = base(
            SONNET,
            json!({"thinking": {"type": "enabled", "budget_tokens": 8000},
                   "tools": [{"name": "Read", "description": "Read a file", "input_schema": {"type": "object", "properties": {"file_path": {"type": "string"}}, "required": ["file_path"]}}],
                   "messages": [
                {"role": "user", "content": "Read Cargo.toml."},
                {"role": "assistant", "content": [{"type": "text", "text": "Reading it."},
                                                  {"type": "tool_use", "id": "toolu_01A", "name": "Read", "input": {"file_path": "Cargo.toml"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_01A", "content": "[workspace]"}]},
                {"role": "assistant", "content": [{"type": "thinking", "thinking": "Now the lock file.", "signature": "CiQBKzxNBQYHCAkKCwwNDg8="},
                                                  {"type": "tool_use", "id": "toolu_01B", "name": "Read", "input": {"file_path": "Cargo.lock"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_01B", "content": "version = 4"}]}]}),
        );
        let edits = run(&anthropic(SONNET), &input);
        // 9router puts a signed "." thinking block in front of both tool-use turns; we only
        // remove the foreign one.
        assert_eq!(reasons(&edits), vec![("messages[3].content[0]".into(), Reason::ForeignBlock)]);
        let out = apply(&input, &edits);
        assert_eq!(out["messages"][1], input["messages"][1]);
        assert_eq!(out["messages"][3]["content"], json!([{"type": "tool_use", "id": "toolu_01B", "name": "Read", "input": {"file_path": "Cargo.lock"}}]));
    }

    #[test]
    fn empty_text_blocks_and_empty_messages_are_removed() {
        let input = base(
            SONNET,
            json!({"messages": [
                {"role": "user", "content": "   "},
                {"role": "user", "content": [{"type": "text", "text": ""}, {"type": "text", "text": "Hello."}]},
                {"role": "assistant", "content": []},
                {"role": "assistant", "content": [{"type": "text", "text": " \n"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Hi."}, {"type": "text", "text": ""}]},
                {"role": "user", "content": "Bye."}]}),
        );
        let want = base(
            SONNET,
            json!({"messages": [
                {"role": "user", "content": [{"type": "text", "text": "Hello."}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Hi."}]},
                {"role": "user", "content": "Bye."}]}),
        );
        let edits = run(&anthropic(SONNET), &input);
        assert_eq!(apply(&input, &edits), want);
        assert!(edits.edits.iter().all(|e| e.reason == Reason::EmptyAfterRemoval && e.op == Op::Remove));
        // Same on any target.
        assert_eq!(apply(&input, &run(&openrouter(), &input)), want);
    }

    #[test]
    fn built_in_tools_with_an_mcp_equivalent_are_removed() {
        let tool = |name: &str, d: &str| json!({"name": name, "description": d, "input_schema": {"type": "object", "properties": {}}});
        let msgs = json!([{"role": "user", "content": "Find the docs for axum 0.8."}]);
        let all = [
            tool("Read", "Read a file"),
            tool("WebSearch", "Search the web"),
            tool("WebFetch", "Fetch a URL"),
            tool("mcp__workspace__web_fetch", "Fetch a URL"),
            tool("mcp__exa__web_search_exa", "Exa search"),
            tool("mcp__browsermcp__browser_navigate", "Navigate"),
            tool("mcp__Claude_in_Chrome__navigate", "Navigate"),
            tool("mcp__tavily__tavily_crawl", "Tavily crawl"),
        ];
        let input = base(SONNET, json!({"tools": all, "messages": msgs}));
        let kept = [all[0].clone(), all[4].clone(), all[5].clone(), all[7].clone()];
        let want = base(SONNET, json!({"tools": kept, "messages": msgs}));
        let edits = run(&anthropic(SONNET), &input);
        assert_eq!(apply(&input, &edits), want);
        assert!(edits.edits.iter().all(|e| e.reason == Reason::DuplicateTool && e.op == Op::Remove));
        // No trigger, no removal; `function.name` counts as a name.
        let lone = json!({"tools": [tool("WebSearch", "s"), {"type": "function", "function": {"name": "mcp__tavily__tavily_search"}}]});
        assert_eq!(reasons(&run(&openrouter(), &lone)), vec![("tools[0]".into(), Reason::DuplicateTool)]);
        assert!(run(&openrouter(), &json!({"tools": [tool("WebSearch", "s")]})).is_empty());
    }

    // --- Pieces ------------------------------------------------------------------------------

    fn sig(s: &str) -> bool {
        is_valid_claude_signature(Some(&Value::String(s.into())))
    }

    #[test]
    fn claude_signature_predicate_matches_node() {
        // E-form: first decoded byte 0x12.
        assert!(sig("EkAKCAgIGAIqQAECAwQFBgcICQoLDA0ODxAREhMUFRYXGA=="));
        assert!(sig("Ek"));
        assert!(!sig("EwAA"));
        assert!(!sig("CiQBKzxNBQYHCAkKCwwNDg8="));
        // R-form: base64 of an E-form signature.
        assert!(sig("RWtBSw=="));
        assert!(!sig("RXdBQQ=="));
        // Cache prefix and surrounding white space.
        assert!(sig("  model-cache#  EkAK \n"));
        assert!(!sig("Ek#Cg"));
        // Node's decoder skips foreign characters, accepts the URL alphabet, stops at `=`, and
        // reads a UTF-16 code unit by its low byte (U+0145 reads as `E`).
        assert!(sig("E!k"));
        assert!(sig("Ek-_"));
        assert!(!sig("E=k"));
        assert!(!sig("E"));
        assert!(!sig("E\u{0145}k"));
        assert!(sig("E\u{e9}k"));
        // Missing, empty, or not a string.
        assert!(!is_valid_claude_signature(None));
        assert!(!is_valid_claude_signature(Some(&json!(18))));
        assert!(!sig(""));
        assert!(!sig("   "));
    }

    #[test]
    fn native_server_tool_ids() {
        assert!(is_native_server_tool_id("srvtoolu_01WYG3ziw53XMcoyKL4XcZmE"));
        assert!(is_native_server_tool_id("srvtoolu__"));
        assert!(!is_native_server_tool_id("srvtoolu_"));
        assert!(!is_native_server_tool_id("srvtoolu_a-b"));
        assert!(!is_native_server_tool_id("call_9f2c41d7"));
        assert!(!is_native_server_tool_id(""));
    }

    #[test]
    fn javascript_semantics() {
        assert_eq!(js_trim("\u{feff} a \u{3000}"), "a");
        assert_eq!(js_trim("\u{85}a"), "\u{85}a");
        assert_eq!(js_string(&json!([1, null, "x"])), "1,,x");
        assert_eq!(js_string(&json!({})), "[object Object]");
        assert_eq!(js_string_or_empty(Some(&Value::Null)), "");
        assert!(!js_truthy(&json!(0)) && !js_truthy(&json!("")) && js_truthy(&json!([])));
    }

    #[test]
    fn nothing_to_do_means_no_edits() {
        assert!(run(&anthropic(SONNET), &json!({})).is_empty());
        let plain = base(SONNET, json!({"messages": [{"role": "user", "content": "hi"}, {"role": "assistant", "content": [{"type": "text", "text": "yo"}]}]}));
        assert!(run(&anthropic(SONNET), &plain).is_empty());
        // Messages that aren't objects, and content of other shapes, are left alone.
        let odd = json!({"messages": ["x", null, {"role": "user"}, {"role": "user", "content": 3}]});
        assert!(run(&anthropic(SONNET), &odd).is_empty());
    }

    #[test]
    fn a_fold_into_a_turn_without_content_replaces_the_turn() {
        let input = json!({"messages": [{"role": "user", "name": "n"}, {"role": "system", "content": "s"}]});
        let edits = run(&anthropic(SONNET), &input);
        let want = json!({"messages": [{"role": "user", "name": "n", "content": [{"type": "text", "text": "s"}]}]});
        assert_eq!(apply(&input, &edits), want);
    }
}
