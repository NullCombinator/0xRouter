//! Rendering and reverse-matching the registry's typed templates
//! (contracts/api-style-schema.md § Mapping vocabulary).
//!
//! Rendering fills placeholders from a [`Lookup`]. Matching is the inverse: literal parts
//! must be equal and placeholders are bound to what stands in their place. Match rules:
//! - an object matches when every non-null literal matches and every required
//!   placeholder is present and non-null; keys the template doesn't name are ignored;
//! - a literal `{null}` means "absent or null";
//! - a one-element array template means "for each element": it yields one binding set per
//!   matching element;
//! - an interpolated string splits on its literals (the gate rules out adjacent holes).

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde_json::{Map, Number, Value};
use zerorouter_registry::schema::MatchRule;
use zerorouter_registry::template::{FieldPath, PathSeg, Piece, Template};

/// Where rendering reads placeholder values.
pub trait Lookup {
    fn lookup(&self, name: &str) -> Option<Cow<'_, Value>>;
}

/// Placeholder name → value: a render context, or the result of a match.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Bindings(pub BTreeMap<String, Value>);

impl Bindings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, name: &str, v: impl Into<Value>) -> Self {
        self.0.insert(name.to_owned(), v.into());
        self
    }

    pub fn set(&mut self, name: &str, v: impl Into<Value>) {
        self.0.insert(name.to_owned(), v.into());
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    pub fn str(&self, name: &str) -> Option<&str> {
        self.0.get(name)?.as_str()
    }

    pub fn u64(&self, name: &str) -> Option<u64> {
        let v = self.0.get(name)?;
        v.as_u64().or_else(|| v.as_str()?.parse().ok())
    }
}

impl Lookup for Bindings {
    fn lookup(&self, name: &str) -> Option<Cow<'_, Value>> {
        self.0.get(name).map(Cow::Borrowed)
    }
}

impl<F: Fn(&str) -> Option<Value>> Lookup for F {
    fn lookup(&self, name: &str) -> Option<Cow<'_, Value>> {
        self(name).map(Cow::Owned)
    }
}

/// Two lookups, the first winning.
pub struct Chain<'a>(pub &'a dyn Lookup, pub &'a dyn Lookup);

impl Lookup for Chain<'_> {
    fn lookup(&self, name: &str) -> Option<Cow<'_, Value>> {
        self.0.lookup(name).or_else(|| self.1.lookup(name))
    }
}

// ── Render ─────────────────────────────────────────────────────────────────────

/// Renders `t`. A whole-string placeholder keeps the value's type; an absent required
/// one renders as null; an absent `{p?}` omits its key (or array element).
pub fn render(t: &Template, ctx: &dyn Lookup) -> Value {
    render_opt(t, ctx).unwrap_or(Value::Null)
}

fn render_opt(t: &Template, ctx: &dyn Lookup) -> Option<Value> {
    Some(match t {
        Template::Null => Value::Null,
        Template::Bool(b) => Value::Bool(*b),
        Template::Int(n) => Value::from(*n),
        Template::Float(f) => Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Template::Str(s) => Value::String(s.clone()),
        Template::Interp(pieces) => {
            let mut out = String::new();
            for p in pieces {
                match p {
                    Piece::Lit(l) => out.push_str(l),
                    Piece::Hole(name) => {
                        if let Some(v) = ctx.lookup(name) {
                            push_scalar(&mut out, &v);
                        }
                    }
                }
            }
            Value::String(out)
        }
        Template::Hole { name, optional } => match ctx.lookup(name) {
            Some(v) => v.into_owned(),
            None if *optional => return None,
            None => Value::Null,
        },
        Template::Array(items) => Value::Array(items.iter().filter_map(|i| render_opt(i, ctx)).collect()),
        Template::Object(fields) => {
            Value::Object(fields.iter().filter_map(|(k, v)| Some((k.clone(), render_opt(v, ctx)?))).collect())
        }
    })
}

fn push_scalar(out: &mut String, v: &Value) {
    match v {
        Value::String(s) => out.push_str(s),
        Value::Null => {}
        other => out.push_str(&other.to_string()),
    }
}

// ── Match ──────────────────────────────────────────────────────────────────────

/// The first binding set under which `v` matches `t`.
pub fn match_value(t: &Template, v: &Value) -> Option<Bindings> {
    match_all(t, v).into_iter().next()
}

/// Every binding set under which `v` matches `t` (more than one only through
/// "for each element" arrays).
pub fn match_all(t: &Template, v: &Value) -> Vec<Bindings> {
    matches(t, Some(v), vec![Bindings::new()])
}

fn matches(t: &Template, v: Option<&Value>, acc: Vec<Bindings>) -> Vec<Bindings> {
    if acc.is_empty() {
        return acc;
    }
    let fail = Vec::new;
    match t {
        Template::Null => match v {
            None | Some(Value::Null) => acc,
            _ => fail(),
        },
        Template::Bool(b) => keep(v == Some(&Value::Bool(*b)), acc),
        Template::Int(n) => keep(v.and_then(Value::as_f64) == Some(*n as f64), acc),
        Template::Float(f) => keep(v.and_then(Value::as_f64) == Some(*f), acc),
        Template::Str(s) => keep(v.and_then(Value::as_str) == Some(s.as_str()), acc),
        Template::Hole { name, optional } => match v {
            None | Some(Value::Null) if *optional => acc,
            None | Some(Value::Null) => fail(),
            Some(v) => bind_all(acc, |b| b.set(name, v.clone())),
        },
        Template::Interp(pieces) => {
            let Some(s) = v.and_then(Value::as_str) else { return fail() };
            match split_interp(pieces, s) {
                Some(found) => bind_all(acc, |b| {
                    for (name, part) in &found {
                        b.set(name, *part);
                    }
                }),
                None => fail(),
            }
        }
        Template::Array(items) => {
            let Some(arr) = v.and_then(Value::as_array) else { return fail() };
            match items.as_slice() {
                [each] => arr.iter().flat_map(|el| matches(each, Some(el), acc.clone())).collect(),
                _ if items.len() == arr.len() => {
                    items.iter().zip(arr).fold(acc, |acc, (t, el)| matches(t, Some(el), acc))
                }
                _ => fail(),
            }
        }
        Template::Object(fields) => {
            let Some(obj) = v.and_then(Value::as_object) else {
                return if matches!(v, None | Some(Value::Null)) && optional(t) { acc } else { fail() };
            };
            fields.iter().fold(acc, |acc, (k, t)| matches(t, obj.get(k), acc))
        }
    }
}

/// Whether `t` binds nothing required: an optional hole, or an object of only those. Such
/// an object may be absent (`usage.prompt_tokens_details` on a provider that omits it).
fn optional(t: &Template) -> bool {
    match t {
        Template::Hole { optional, .. } => *optional,
        Template::Object(fields) => !fields.is_empty() && fields.iter().all(|(_, t)| optional(t)),
        _ => false,
    }
}

/// The paths of object keys in `v` that `t` has no field for, at any depth, each under
/// `at`. A placeholder takes its whole value, so nothing under it is reported. Call it on
/// a value `t` already matched.
pub fn unmatched_keys(t: &Template, v: &Value, at: &str, out: &mut Vec<String>) {
    match (t, v) {
        (Template::Object(fields), Value::Object(obj)) => {
            for (k, val) in obj {
                let here = if at.is_empty() { k.clone() } else { format!("{at}.{k}") };
                match fields.iter().find(|(f, _)| f == k) {
                    Some((_, ft)) => unmatched_keys(ft, val, &here, out),
                    None => out.push(here),
                }
            }
        }
        (Template::Array(items), Value::Array(arr)) => {
            for (i, el) in arr.iter().enumerate() {
                let each = if let [each] = items.as_slice() { Some(each) } else { items.get(i) };
                if let Some(et) = each {
                    unmatched_keys(et, el, &format!("{at}[{i}]"), out);
                }
            }
        }
        _ => {}
    }
}

fn keep(ok: bool, acc: Vec<Bindings>) -> Vec<Bindings> {
    if ok { acc } else { Vec::new() }
}

fn bind_all(mut acc: Vec<Bindings>, f: impl Fn(&mut Bindings)) -> Vec<Bindings> {
    acc.iter_mut().for_each(f);
    acc
}

/// Splits `s` on the literal pieces, binding each hole to the text between them.
fn split_interp<'a>(pieces: &'a [Piece], mut s: &'a str) -> Option<Vec<(&'a str, &'a str)>> {
    let mut out = Vec::new();
    let mut pending: Option<&str> = None;
    for p in pieces {
        match p {
            Piece::Lit(l) => match pending.take() {
                Some(name) => {
                    let at = s.find(l.as_str())?;
                    out.push((name, &s[..at]));
                    s = &s[at + l.len()..];
                }
                None => s = s.strip_prefix(l.as_str())?,
            },
            Piece::Hole(name) => pending = Some(name),
        }
    }
    match pending {
        Some(name) => out.push((name, s)),
        None if !s.is_empty() => return None,
        None => {}
    }
    Some(out)
}

// ── Selectors ──────────────────────────────────────────────────────────────────

/// The values `path` selects in `v`, in document order.
pub fn select<'v>(path: &FieldPath, v: &'v Value) -> Vec<&'v Value> {
    let mut cur = vec![v];
    for seg in &path.0 {
        cur = cur
            .into_iter()
            .flat_map(|v| -> Vec<&Value> {
                match seg {
                    PathSeg::Key(k) => v.get(k).into_iter().collect(),
                    PathSeg::Index(i) => v.get(*i).into_iter().collect(),
                    PathSeg::Each => v.as_array().map(|a| a.iter().collect()).unwrap_or_default(),
                }
            })
            .collect();
    }
    cur
}

/// The first value `path` selects, if any.
pub fn select_one<'v>(path: &FieldPath, v: &'v Value) -> Option<&'v Value> {
    select(path, v).into_iter().next()
}

/// Sets the value at `path` (keys and indexes only), creating objects on the way.
/// Returns false when an existing non-object value is in the way.
pub fn set_path(target: &mut Value, path: &FieldPath, value: Value) -> bool {
    let Some((last, init)) = path.0.split_last() else {
        *target = value;
        return true;
    };
    let mut cur = target;
    for seg in init {
        cur = match seg {
            PathSeg::Key(k) => {
                if cur.is_null() {
                    *cur = Value::Object(Map::new());
                }
                let Some(obj) = cur.as_object_mut() else { return false };
                obj.entry(k.clone()).or_insert(Value::Null)
            }
            PathSeg::Index(i) => match cur.get_mut(*i) {
                Some(v) => v,
                None => return false,
            },
            PathSeg::Each => return false,
        };
    }
    match last {
        PathSeg::Key(k) => {
            if cur.is_null() {
                *cur = Value::Object(Map::new());
            }
            match cur.as_object_mut() {
                Some(obj) => {
                    obj.insert(k.clone(), value);
                    true
                }
                None => false,
            }
        }
        PathSeg::Index(i) => match cur.get_mut(*i) {
            Some(v) => {
                *v = value;
                true
            }
            None => false,
        },
        PathSeg::Each => false,
    }
}

/// Removes the value at `path` (keys only); returns it.
pub fn take_path(target: &mut Value, path: &FieldPath) -> Option<Value> {
    let (last, init) = path.0.split_last()?;
    let mut cur = target;
    for seg in init {
        cur = match seg {
            PathSeg::Key(k) => cur.get_mut(k)?,
            PathSeg::Index(i) => cur.get_mut(*i)?,
            PathSeg::Each => return None,
        };
    }
    match last {
        PathSeg::Key(k) => cur.as_object_mut()?.remove(k),
        _ => None,
    }
}

/// Whether `rule`'s body conditions hold for `body`. A rule with none holds.
pub fn body_rule_holds(rule: &MatchRule, body: &Value) -> bool {
    let at = |p: &str| FieldPath::parse(p).ok().and_then(|fp| select_one(&fp, body).cloned());
    let present = rule.path_present.as_deref().is_none_or(|p| at(p).is_some_and(|v| !v.is_null()));
    let equals = rule.path_equals.as_deref().is_none_or(|pe| match pe {
        [p, want] => at(p).is_some_and(|v| v.as_str().map_or_else(|| &v.to_string() == want, |s| s == want)),
        _ => false,
    });
    present && equals
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn t(src: &str) -> Template {
        let v: toml::Value = toml::from_str(&format!("v = {src}")).unwrap();
        Template::parse(&v["v"]).unwrap()
    }

    #[test]
    fn render_keeps_types_and_omits_optional() {
        let ctx = Bindings::new().with("block.index", 2).with("delta.text", "hi").with("id", "abc");
        let got = render(
            &t(
                r#"{ index = "{block.index}", delta = { text = "{delta.text}" }, x = "{usage.input?}", id = "msg_{id}", n = "{null}" }"#,
            ),
            &ctx,
        );
        assert_eq!(got, json!({ "index": 2, "delta": { "text": "hi" }, "id": "msg_abc", "n": null }));
        assert_eq!(render(&t(r#""{{x}}""#), &ctx), json!("{x}"));
    }

    #[test]
    fn match_extracts_and_ignores_extra_keys() {
        let tpl = t(
            r#"{ type = "content_block_delta", index = "{block.index}", delta = { type = "text_delta", text = "{delta.text}" } }"#,
        );
        let v = json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "yo" }, "extra": 1 });
        let b = match_value(&tpl, &v).unwrap();
        assert_eq!(b.get("block.index"), Some(&json!(0)));
        assert_eq!(b.str("delta.text"), Some("yo"));
        let wrong = json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{" } });
        assert!(match_value(&tpl, &wrong).is_none());
    }

    #[test]
    fn unmatched_keys_are_reported_at_any_depth_but_not_under_a_placeholder() {
        let tpl =
            t(r#"{ type = "function", function = { name = "{n}", parameters = "{p}" }, list = [{ id = "{i}" }] }"#);
        let v = json!({
            "type": "function", "x_top": 1,
            "function": { "name": "f", "strict": true, "parameters": { "anything": { "goes": 1 } } },
            "list": [{ "id": 1 }, { "id": 2, "x_el": 0 }]
        });
        let mut out = Vec::new();
        unmatched_keys(&tpl, &v, "tools[0]", &mut out);
        assert_eq!(out, ["tools[0].x_top", "tools[0].function.strict", "tools[0].list[1].x_el"]);
    }

    #[test]
    fn null_literal_means_absent_or_null_and_required_holes_must_be_present() {
        let tpl = t(r#"{ choices = [{ delta = { content = "{delta.text}" }, finish_reason = "{null}" }] }"#);
        assert!(match_value(&tpl, &json!({ "choices": [{ "delta": { "content": "a" } }] })).is_some());
        assert!(
            match_value(&tpl, &json!({ "choices": [{ "delta": { "content": "a" }, "finish_reason": null }] }))
                .is_some()
        );
        assert!(
            match_value(&tpl, &json!({ "choices": [{ "delta": { "content": "a" }, "finish_reason": "stop" }] }))
                .is_none()
        );
        assert!(match_value(&tpl, &json!({ "choices": [{ "delta": { "content": null } }] })).is_none());
        let opt = t(r#"{ a = "{x?}" }"#);
        assert_eq!(match_value(&opt, &json!({})), Some(Bindings::new()));
        let nested = t(r#"{ usage = { n = "{u.n?}", d = { c = "{u.c?}" } }, k = { kind = "x", v = "{v?}" } }"#);
        assert!(
            match_value(&nested, &json!({ "usage": { "n": 1 }, "k": { "kind": "x" } })).is_some(),
            "an all-optional object may be absent"
        );
        assert!(match_value(&nested, &json!({ "usage": { "n": 1 } })).is_none(), "an object with a literal may not");
    }

    #[test]
    fn one_element_array_matches_each_element() {
        let tpl = t(r#"{ parts = [{ text = "{part.text}" }] }"#);
        let all = match_all(&tpl, &json!({ "parts": [{ "text": "a" }, { "inline": 1 }, { "text": "b" }] }));
        let texts: Vec<_> = all.iter().map(|b| b.str("part.text").unwrap()).collect();
        assert_eq!(texts, ["a", "b"]);
    }

    #[test]
    fn interpolation_reverses() {
        let tpl = t(r#""data:{media.mime};base64,{media.data}""#);
        let b = match_value(&tpl, &json!("data:image/png;base64,AAAA")).unwrap();
        assert_eq!((b.str("media.mime"), b.str("media.data")), (Some("image/png"), Some("AAAA")));
        assert!(match_value(&tpl, &json!("https://x")).is_none());
    }

    #[test]
    fn render_then_match_round_trips() {
        let tpl = t(
            r#"{ id = "chatcmpl-{response.id}", model = "{response.model}", choices = [{ index = 0, finish_reason = "{response.finish}" }] }"#,
        );
        let ctx = Bindings::new().with("response.id", "x1").with("response.model", "m").with("response.finish", "stop");
        assert_eq!(match_value(&tpl, &render(&tpl, &ctx)), Some(ctx));
    }

    #[test]
    fn selectors_and_paths() {
        let v = json!({ "a": [{ "b": 1 }, { "b": 2 }], "c": { "d": "x" } });
        let each = FieldPath::parse("a[*].b").unwrap();
        assert_eq!(select(&each, &v), [&json!(1), &json!(2)]);
        assert_eq!(select_one(&FieldPath::parse("c.d").unwrap(), &v), Some(&json!("x")));
        let mut out = Value::Null;
        assert!(set_path(&mut out, &FieldPath::parse("generationConfig.maxOutputTokens").unwrap(), json!(5)));
        assert_eq!(out, json!({ "generationConfig": { "maxOutputTokens": 5 } }));
        assert_eq!(take_path(&mut out, &FieldPath::parse("generationConfig.maxOutputTokens").unwrap()), Some(json!(5)));
    }
}
