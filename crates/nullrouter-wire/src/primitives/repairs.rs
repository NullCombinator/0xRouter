//! Cross-style repairs (contracts/api-style-schema.md § Named primitives). They run only
//! when the client style differs from the wire, and touch tool ids, names and schemas,
//! never message text.

use std::collections::BTreeSet;

use nullrouter_registry::schema::Repair;
use serde_json::{Map, Value};

use crate::ir::{Message, Part, Request, ResultContent, Role, ToolChoice};

pub fn apply(r: Repair, req: &mut Request) {
    match r {
        Repair::EnsureToolCallIds => ensure_tool_call_ids(req),
        Repair::FillMissingToolResults => fill_missing_tool_results(req),
        Repair::GeminiSchemaSanitize => {
            for t in &mut req.tools {
                sanitize_schema(&mut t.parameters);
            }
        }
        Repair::GeminiFunctionNameSanitize => sanitize_function_names(req),
    }
}

fn valid_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// 9router `ensureToolCallIds`: ids keep only `[A-Za-z0-9_-]`; an empty one is minted from
/// its position and tool name. A result with no id takes the oldest open call's id,
/// preferring one with the same name.
fn ensure_tool_call_ids(req: &mut Request) {
    let mut open: Vec<(String, String)> = Vec::new();
    for (i, m) in req.messages.iter_mut().enumerate() {
        let mut j = 0;
        for p in &mut m.parts {
            match p {
                Part::ToolCall { id, name, .. } => {
                    id.retain(valid_id_char);
                    if id.is_empty() {
                        let n: String = name.chars().filter(|c| valid_id_char(*c)).collect();
                        *id =
                            if n.is_empty() { format!("call_msg{i}_tc{j}") } else { format!("call_msg{i}_tc{j}_{n}") };
                    }
                    open.push((id.clone(), name.clone()));
                    j += 1;
                }
                Part::ToolResult { id, name, .. } => {
                    id.retain(valid_id_char);
                    if id.is_empty() {
                        let at = open
                            .iter()
                            .position(|(_, n)| Some(n) == name.as_ref())
                            .or_else(|| (!open.is_empty()).then_some(0));
                        if let Some(at) = at {
                            *id = open.remove(at).0;
                        }
                    } else {
                        open.retain(|(o, _)| o != id);
                    }
                }
                _ => {}
            }
        }
    }
}

/// 9router `fixMissingToolResponses`: when the message after an assistant tool call
/// doesn't answer it, an empty result is inserted.
fn fill_missing_tool_results(req: &mut Request) {
    let mut i = 0;
    while i < req.messages.len() {
        let calls: Vec<(String, String)> = req.messages[i]
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall { id, name, .. } => Some((id.clone(), name.clone())),
                _ => None,
            })
            .collect();
        let Some(next) = req.messages.get(i + 1).filter(|_| !calls.is_empty()) else {
            i += 1;
            continue;
        };
        let answered: BTreeSet<&str> = next
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolResult { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let missing: Vec<Part> = calls
            .into_iter()
            .filter(|(id, _)| !answered.contains(id.as_str()))
            .map(|(id, name)| Part::ToolResult {
                id,
                name: Some(name),
                content: ResultContent::Text(String::new()),
                is_error: false,
                cache_control: None,
            })
            .collect();
        if !missing.is_empty() {
            req.messages.insert(i + 1, Message { role: Role::Tool, parts: missing });
        }
        i += 1;
    }
}

/// 9router `sanitizeGeminiFunctionName`: `[A-Za-z0-9_.:-]`, a letter or `_` first, 64 chars.
pub fn gemini_function_name(name: &str) -> String {
    if name.is_empty() {
        return "_unknown".into();
    }
    let mut s: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || "_.:-".contains(c) { c } else { '_' }).collect();
    if !s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        s.insert(0, '_');
    }
    s.chars().take(64).collect()
}

fn sanitize_function_names(req: &mut Request) {
    for t in &mut req.tools {
        t.name = gemini_function_name(&t.name);
    }
    if let Some(ToolChoice::Named(n)) = &mut req.tool_choice {
        *n = gemini_function_name(n);
    }
    for p in req.messages.iter_mut().flat_map(|m| m.parts.iter_mut()) {
        match p {
            Part::ToolCall { name, .. } => *name = gemini_function_name(name),
            Part::ToolResult { name: Some(name), .. } => *name = gemini_function_name(name),
            _ => {}
        }
    }
}

/// 9router `UNSUPPORTED_SCHEMA_CONSTRAINTS` (`translator/formats/gemini.js`).
const UNSUPPORTED: &[&str] = &[
    "minLength",
    "maxLength",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minItems",
    "maxItems",
    "format",
    "multipleOf",
    "uniqueItems",
    "contains",
    "unevaluatedProperties",
    "unevaluatedItems",
    "contentSchema",
    "prefixItems",
    "additionalItems",
    "default",
    "examples",
    "$schema",
    "$defs",
    "definitions",
    "const",
    "$ref",
    "$comment",
    "deprecated",
    "readOnly",
    "writeOnly",
    "additionalProperties",
    "propertyNames",
    "patternProperties",
    "enumDescriptions",
    "anyOf",
    "oneOf",
    "allOf",
    "not",
    "dependencies",
    "dependentSchemas",
    "dependentRequired",
    "title",
    "optional",
    "if",
    "then",
    "else",
    "contentMediaType",
    "contentEncoding",
    "cornerRadius",
    "fillColor",
    "fontFamily",
    "fontSize",
    "fontWeight",
    "gap",
    "padding",
    "strokeColor",
    "strokeThickness",
    "textColor",
];

/// Reduces a JSON schema to what gemini accepts: `allOf` merged, `anyOf`/`oneOf` flattened
/// to the first non-null branch, type arrays to their first non-null type, then the
/// unsupported keywords removed. Property names are never touched.
pub fn sanitize_schema(v: &mut Value) {
    let Value::Object(o) = v else { return };
    if let Some(Value::Array(all)) = o.remove("allOf") {
        for mut branch in all {
            sanitize_schema(&mut branch);
            if let Value::Object(b) = branch {
                merge_schema(o, b);
            }
        }
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(branches)) = o.remove(key) {
            let pick = branches.into_iter().find(|b| b.get("type").and_then(Value::as_str) != Some("null"));
            if let Some(Value::Object(b)) = pick {
                for (k, val) in b {
                    o.entry(k).or_insert(val);
                }
            }
        }
    }
    if let Some(Value::Array(types)) = o.get("type") {
        let first = types.iter().find(|t| t.as_str() != Some("null")).cloned();
        o.insert("type".into(), first.unwrap_or_else(|| "string".into()));
    }
    if o.contains_key("properties") && !o.contains_key("type") {
        o.insert("type".into(), "object".into());
    }
    for k in UNSUPPORTED {
        o.remove(*k);
    }
    if let Some(Value::Object(props)) = o.get_mut("properties") {
        props.values_mut().for_each(sanitize_schema);
    }
    if let Some(items) = o.get_mut("items") {
        match items {
            Value::Array(a) => a.iter_mut().for_each(sanitize_schema),
            other => sanitize_schema(other),
        }
    }
    let props: Option<BTreeSet<String>> =
        o.get("properties").and_then(Value::as_object).map(|p| p.keys().cloned().collect());
    if let (Some(props), Some(Value::Array(req))) = (props, o.get_mut("required")) {
        req.retain(|f| f.as_str().is_some_and(|f| props.contains(f)));
        if req.is_empty() {
            o.remove("required");
        }
    }
}

fn merge_schema(into: &mut Map<String, Value>, from: Map<String, Value>) {
    for (k, v) in from {
        match (into.get_mut(&k), v) {
            (Some(Value::Object(a)), Value::Object(b)) if k == "properties" => a.extend(b),
            (Some(Value::Array(a)), Value::Array(b)) if k == "required" => a.extend(b),
            (Some(_), _) => {}
            (None, v) => {
                into.insert(k, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ir::Tool;

    fn call(id: &str, name: &str) -> Part {
        Part::ToolCall { id: id.into(), name: name.into(), arguments: json!({}), cache_control: None }
    }

    #[test]
    fn ids_are_minted_and_results_follow() {
        let mut req = Request {
            messages: vec![
                Message { role: Role::Assistant, parts: vec![call("", "get weather"), call("a.b", "x")] },
                Message {
                    role: Role::Tool,
                    parts: vec![Part::ToolResult {
                        id: String::new(),
                        name: Some("get weather".into()),
                        content: ResultContent::Text("sun".into()),
                        is_error: false,
                        cache_control: None,
                    }],
                },
            ],
            ..Request::default()
        };
        apply(Repair::EnsureToolCallIds, &mut req);
        apply(Repair::FillMissingToolResults, &mut req);
        let ids: Vec<&str> = req
            .messages
            .iter()
            .flat_map(|m| &m.parts)
            .filter_map(|p| match p {
                Part::ToolCall { id, .. } | Part::ToolResult { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        // The inserted result for `ab` comes straight after the calls.
        assert_eq!(ids, ["call_msg0_tc0_getweather", "ab", "ab", "call_msg0_tc0_getweather"]);
        assert_eq!(req.messages.iter().map(|m| m.role).collect::<Vec<_>>(), [Role::Assistant, Role::Tool, Role::Tool]);
    }

    #[test]
    fn schema_sanitize_keeps_property_names() {
        let mut t = Tool {
            name: "1 bad name!".into(),
            description: None,
            parameters: json!({
                "$schema": "x", "additionalProperties": false,
                "properties": { "format": { "type": ["string", "null"], "format": "date" },
                                "v": { "anyOf": [{ "type": "null" }, { "type": "integer", "minimum": 0 }] } },
                "required": ["format", "gone"]
            }),
            cache_control: None,
        };
        sanitize_schema(&mut t.parameters);
        assert_eq!(
            t.parameters,
            json!({
                "type": "object",
                "properties": { "format": { "type": "string" }, "v": { "type": "integer", "minimum": 0 } },
                "required": ["format"]
            })
        );
        assert_eq!(gemini_function_name(&t.name), "_1_bad_name_");
    }
}
