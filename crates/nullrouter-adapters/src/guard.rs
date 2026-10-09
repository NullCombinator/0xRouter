//! The guardrail (research R6): a pure check that a third-party adapter's output adds or
//! changes no tool call, tool definition, tool result, opaque block or unplaced key. Removals
//! pass; additions and changes are violations.
//!
//! The check compares the IR the client style decodes from the body before and after the
//! edits, as multisets: the after-side must be a sub-multiset of the before-side. A body that
//! no longer decodes is not a violation but a failure ([`Verdict::Undecodable`]): nothing
//! edited leaves 0router, so the adapter is not marked suspect.
//!
//! Violations carry the rule and IR locators (`messages[1].parts[0]`, `tools[2]`, an unplaced
//! key's path), never a value (FR-025). The request IR holds the only opaque blocks and
//! unplaced keys, so the response and event checks cover tool calls alone.

use std::collections::BTreeMap;

use nullrouter_wire::codec::{Style, request, response};
use nullrouter_wire::ir::{BlockKind, Event, Part, ResultContent, Tool};
use nullrouter_wire::ir::{Request, Response};
use nullrouter_wire::stream::{Frame, StreamReader};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::record::GuardrailRule;

/// The most locators a violation names.
pub const MAX_PATHS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Violation {
        rule: GuardrailRule,
        paths: Vec<String>,
    },
    /// The edited body no longer decodes in the client style.
    Undecodable,
}

/// One member of a multiset: what it is called, a digest of all of it, and where it sits.
struct Item {
    name: String,
    digest: String,
    at: String,
}

#[derive(Default)]
struct Groups {
    calls: Vec<Item>,
    defs: Vec<Item>,
    results: Vec<Item>,
    opaque: Vec<Item>,
    unplaced: Vec<Item>,
    /// The request's `tool_choice`: removing it forces no call, so it may go (the same
    /// sub-multiset rule as the other groups), but it may not appear or change.
    choice: Vec<Item>,
}

/// Checks the edited request body against the IR decoded from the original. The bodies are
/// needed because an unplaced key is digested with its value, which the IR does not hold.
pub fn check_request(style: &Style, before: &Request, before_body: &Value, after_body: &Value) -> Verdict {
    let Ok(after) = request::decode(style, after_body) else { return Verdict::Undecodable };
    let (b, a) = (request_groups(before, before_body), request_groups(&after, after_body));
    use GuardrailRule as R;
    first_violation(&[
        check(&b.calls, &a.calls, R::ToolCallAdded, R::ToolCallChanged),
        check(&b.defs, &a.defs, R::ToolDefAdded, R::ToolDefChanged),
        check(&b.results, &a.results, R::ToolResultAdded, R::ToolResultChanged),
        check(&b.opaque, &a.opaque, R::OpaqueAdded, R::OpaqueAdded),
        check(&b.unplaced, &a.unplaced, R::UnplacedAdded, R::UnplacedAdded),
        check(&b.choice, &a.choice, R::ToolCallAdded, R::ToolCallChanged),
    ])
}

/// Checks an edited non-stream answer against the IR decoded from the original.
pub fn check_response(style: &Style, before: &Response, after_body: &Value) -> Verdict {
    let Ok(after) = response::decode(style, after_body) else { return Verdict::Undecodable };
    let (b, a) = (response_calls(before), response_calls(&after));
    first_violation(&[check(&b, &a, GuardrailRule::ToolCallAdded, GuardrailRule::ToolCallChanged)])
}

/// Checks the events an edited stream event decodes to against those of the original. The
/// tool-call starts and the argument fragments must be the same on both sides: one the original
/// did not carry is a violation, and so is one it did carry that is gone (a stream reader drops
/// an emptied fragment, so removal is a change). Text and thinking deltas may be removed.
pub fn check_event(before: &[Event], after: &[Event]) -> Verdict {
    check_events_at(before, after, "", "")
}

fn check_events_at(before: &[Event], after: &[Event], before_ord: &str, after_ord: &str) -> Verdict {
    use GuardrailRule::{ToolCallAdded as Added, ToolCallChanged as Changed};
    let (b, a) = (event_items(before, before_ord), event_items(after, after_ord));
    first_violation(&[
        check(&b.0, &a.0, Added, Added),
        check(&a.0, &b.0, Changed, Changed),
        check(&b.1, &a.1, Changed, Changed),
        check(&a.1, &b.1, Changed, Changed),
    ])
}

/// Checks one edited client stream event against the original, both read by `style`'s stream
/// reader with no state carried over. A tool-call argument delta read this way opens a call
/// with no id and no name, the same on both sides, so only an added or changed start or
/// fragment is a violation. Either event failing to read is [`Verdict::Undecodable`].
pub fn check_event_frame(style: &Style, before: &Value, after: &Value) -> Verdict {
    let read = |event: &Value| -> Option<(Vec<Event>, String)> {
        let frame = Frame { event: None, data: event.to_string() };
        let mut reader = StreamReader::new(style).ok()?;
        let events = reader.read(&frame).ok()?;
        Some((events, reader.tool_ordinals(&frame)))
    };
    match (read(before), read(after)) {
        (Some((b, bo)), Some((a, ao))) => check_events_at(&b, &a, &bo, &ao),
        _ => Verdict::Undecodable,
    }
}

type Check<'a> = (&'a [Item], &'a [Item], GuardrailRule, GuardrailRule);

fn check<'a>(before: &'a [Item], after: &'a [Item], added: GuardrailRule, changed: GuardrailRule) -> Check<'a> {
    (before, after, added, changed)
}

fn first_violation(checks: &[Check<'_>]) -> Verdict {
    for (before, after, added, changed) in checks {
        if let Some((rule, paths)) = excess(before, after, *added, *changed) {
            return Verdict::Violation { rule, paths };
        }
    }
    Verdict::Ok
}

/// The after-items the before-side cannot account for, as a rule and locators. `changed` when
/// one of them shares a name with a before-item, `added` otherwise.
fn excess(
    before: &[Item],
    after: &[Item],
    added: GuardrailRule,
    changed: GuardrailRule,
) -> Option<(GuardrailRule, Vec<String>)> {
    let mut left: BTreeMap<&str, usize> = BTreeMap::new();
    for i in before {
        *left.entry(&i.digest).or_default() += 1;
    }
    let extra: Vec<&Item> = after
        .iter()
        .filter(|i| match left.get_mut(i.digest.as_str()) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .collect();
    if extra.is_empty() {
        return None;
    }
    let rule = if extra.iter().any(|i| before.iter().any(|b| b.name == i.name)) { changed } else { added };
    Some((rule, extra.iter().take(MAX_PATHS).map(|i| i.at.clone()).collect()))
}

fn request_groups(r: &Request, body: &Value) -> Groups {
    let mut g = Groups::default();
    for (i, p) in r.system.iter().enumerate() {
        part_items(p, &format!("system[{i}]"), &mut g);
    }
    for (i, m) in r.messages.iter().enumerate() {
        for (j, p) in m.parts.iter().enumerate() {
            part_items(p, &format!("messages[{i}].parts[{j}]"), &mut g);
        }
    }
    for (i, t) in r.tools.iter().enumerate() {
        g.defs.push(def_item(t, format!("tools[{i}]")));
    }
    for o in &r.opaque {
        g.opaque.push(Item { name: o.at.clone(), digest: digest(&["opaque", &canon(&o.value)]), at: o.at.clone() });
    }
    for path in &r.unplaced {
        let value = resolve(body, path).map_or_else(|| "\u{0}unresolved".to_owned(), canon);
        g.unplaced.push(Item { name: path.clone(), digest: digest(&["unplaced", path, &value]), at: path.clone() });
    }
    if let Some(c) = &r.tool_choice {
        g.choice.push(Item {
            name: "tool_choice".into(),
            digest: digest(&["tool_choice", &format!("{c:?}")]),
            at: "tool_choice".into(),
        });
    }
    g
}

/// The value an unplaced-key locator (`key`, `a.b`, `messages[0].k`) names in `body`. Keys that
/// themselves hold dots or brackets are tried whole before the path is split.
fn resolve<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(v);
    }
    if let Some(rest) = path.strip_prefix('.') {
        return resolve(v, rest);
    }
    if let Some(rest) = path.strip_prefix('[') {
        let (idx, rest) = rest.split_once(']')?;
        return resolve(v.as_array()?.get(idx.parse::<usize>().ok()?)?, rest);
    }
    v.as_object()?.iter().find_map(|(k, child)| {
        let rest = path.strip_prefix(k.as_str())?;
        if rest.is_empty() || rest.starts_with('.') || rest.starts_with('[') { resolve(child, rest) } else { None }
    })
}

fn part_items(p: &Part, at: &str, g: &mut Groups) {
    match p {
        Part::ToolCall { .. } => g.calls.push(call_item(p, at.to_owned())),
        Part::ToolResult { id, .. } => {
            g.results.push(Item { name: id.clone(), digest: part_digest(p), at: at.to_owned() });
        }
        _ => {}
    }
}

fn response_calls(r: &Response) -> Vec<Item> {
    let calls = r.content.iter().enumerate().filter(|(_, p)| matches!(p, Part::ToolCall { .. }));
    calls.map(|(i, p)| call_item(p, format!("content[{i}]"))).collect()
}

fn call_item(p: &Part, at: String) -> Item {
    let name = if let Part::ToolCall { id, .. } = p { id.clone() } else { String::new() };
    Item { name, digest: part_digest(p), at }
}

fn def_item(t: &Tool, at: String) -> Item {
    let description = t.description.as_deref().unwrap_or_default();
    Item { name: t.name.clone(), digest: digest(&["tool", &t.name, description, &canon(&t.parameters)]), at }
}

/// The tool-call starts and the argument fragments of a run of events. `ordinal` is the
/// provider's tool index the events were read from; it is part of every digest, so moving a
/// fragment to another call is a change.
fn event_items(events: &[Event], ordinal: &str) -> (Vec<Item>, Vec<Item>) {
    let (mut starts, mut args) = (Vec::new(), Vec::new());
    for (i, e) in events.iter().enumerate() {
        match e {
            Event::BlockStart(BlockKind::ToolCall { id, name }) => starts.push(Item {
                name: id.clone(),
                digest: digest(&["start", ordinal, id, name]),
                at: format!("events[{i}]"),
            }),
            Event::ToolArguments(s) => {
                args.push(Item {
                    name: String::new(),
                    digest: digest(&["arguments", ordinal, s]),
                    at: format!("events[{i}]"),
                });
            }
            _ => {}
        }
    }
    (starts, args)
}

/// A digest of everything about a part that reaches a provider, except `cache_control`.
fn part_digest(p: &Part) -> String {
    match p {
        Part::Text { text, .. } => digest(&["text", text]),
        Part::Image { media, .. } => digest(&["image", &format!("{media:?}")]),
        Part::Audio { media, .. } => digest(&["audio", &format!("{media:?}")]),
        Part::ToolCall { id, name, arguments, .. } => digest(&["call", id, name, &canon(arguments)]),
        Part::ToolResult { id, content, is_error, .. } => {
            digest(&["result", id, &is_error.to_string(), &content_digest(content)])
        }
        Part::Thinking { text, signature, vendor } => {
            digest(&["thinking", text, signature.as_deref().unwrap_or_default(), vendor.as_deref().unwrap_or_default()])
        }
    }
}

fn content_digest(c: &ResultContent) -> String {
    match c {
        ResultContent::Text(s) => digest(&["text", s]),
        ResultContent::Json(v) => digest(&["json", &canon(v)]),
        ResultContent::Parts(ps) => {
            let parts: Vec<String> = ps.iter().map(part_digest).collect();
            digest(&parts.iter().map(String::as_str).collect::<Vec<_>>())
        }
    }
}

/// SHA-256 over `fields`, each framed by its length so no two lists collide.
fn digest(fields: &[&str]) -> String {
    let mut h = Sha256::new();
    for f in fields {
        h.update((f.len() as u64).to_le_bytes());
        h.update(f.as_bytes());
    }
    format!("{:x}", h.finalize())
}

/// JSON text with object keys in sorted order, so two spellings of one value compare equal.
fn canon(v: &Value) -> String {
    let mut out = String::new();
    write_canon(v, &mut out);
    out
}

fn write_canon(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (n, k) in keys.into_iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canon(&m[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (n, x) in a.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                write_canon(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}
