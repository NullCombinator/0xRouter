//! The non-text codecs: embeddings, image, TTS, STT and video (research R16).
//!
//! A non-text body maps to a [`TypeValue`]: scalar bindings plus items. Names a template
//! binds inside a "for each" array (a one-element array) are per item: the vectors of an
//! embeddings answer, the images of an image answer. Encoding reverses it:
//! - a "for each" array renders once per item, or once from the scalars when there are
//!   no items (then a scalar array named inside it is iterated instead);
//! - a placeholder outside any "for each" that the scalars lack takes the one item's value,
//!   or an array of every item's value.
//!
//! Decoding ignores literals, which the templates carry for rendering (`object =
//! "embedding"`), so a provider that omits or varies them still decodes.
//!
//! A response template that is exactly `"{output.<name>}"` is a binary body: the bytes
//! travel as base64 in the IR and the content type as `output.mime_type`.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use zerorouter_registry::schema::{BodyEncoding, EmbeddingVector, ModelType, TypeCodec as TypeDecl};
use zerorouter_registry::template::{FieldPath, Template};

use super::CodecError;
use crate::primitives::embeddings;
use crate::template::{self, Bindings, Lookup};

#[derive(Debug, Clone)]
pub struct TypeCodec {
    pub ty: ModelType,
    pub encoding: BodyEncoding,
    pub request: Template,
    pub response: Template,
    pub vector: Option<EmbeddingVector>,
    pub job: Option<JobTpl>,
    /// Name → (request, response).
    pub variants: BTreeMap<String, (Template, Template)>,
}

#[derive(Debug, Clone)]
pub struct JobTpl {
    pub status: Template,
    /// IR status → style status.
    pub status_map: BTreeMap<String, String>,
}

/// IR job states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn done(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

/// A non-text request or answer in the IR.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypeValue {
    pub scalars: Bindings,
    pub items: Vec<Bindings>,
}

impl TypeValue {
    /// A scalar, else the one item's value, else every item's value.
    pub fn get(&self, name: &str) -> Option<Cow<'_, Value>> {
        if let Some(v) = self.scalars.get(name) {
            return Some(Cow::Borrowed(v));
        }
        let vals: Vec<&Value> = self.items.iter().filter_map(|i| i.get(name)).collect();
        match vals.as_slice() {
            [] => None,
            [one] if self.items.len() == 1 => Some(Cow::Borrowed(one)),
            _ => Some(Cow::Owned(Value::Array(vals.into_iter().cloned().collect()))),
        }
    }

    pub fn str(&self, name: &str) -> Option<String> {
        self.get(name).and_then(|v| v.as_str().map(str::to_owned))
    }

    /// Token usage bound under `usage.*`.
    pub fn usage(&self, name: &str) -> Option<u64> {
        self.get(&format!("usage.{name}")).and_then(|v| v.as_u64())
    }
}

impl Lookup for TypeValue {
    fn lookup(&self, name: &str) -> Option<Cow<'_, Value>> {
        self.get(name)
    }
}

fn tpl(v: &toml::Value, at: &str) -> Result<Template, CodecError> {
    Template::parse(v).map_err(|e| CodecError::decode(at, e.to_string()))
}

impl TypeCodec {
    pub fn compile(ty: ModelType, d: &TypeDecl) -> Result<Self, CodecError> {
        let at = ty.as_str();
        Ok(Self {
            ty,
            encoding: d.encoding,
            request: tpl(&d.request, &format!("{at}.request"))?,
            response: tpl(&d.response, &format!("{at}.response"))?,
            vector: d.vector,
            job: d
                .job
                .as_ref()
                .map(|j| {
                    Ok::<_, CodecError>(JobTpl {
                        status: tpl(&j.status, &format!("{at}.job.status"))?,
                        status_map: j.status_map.clone(),
                    })
                })
                .transpose()?,
            variants: d
                .variants
                .iter()
                .map(|(n, v)| {
                    let at = format!("{at}.variants.{n}");
                    Ok((
                        n.clone(),
                        (tpl(&v.request, &format!("{at}.request"))?, tpl(&v.response, &format!("{at}.response"))?),
                    ))
                })
                .collect::<Result<_, CodecError>>()?,
        })
    }

    /// This codec with a route's variant in place of the main shapes.
    pub fn variant(&self, name: Option<&str>) -> Cow<'_, Self> {
        match name.and_then(|n| self.variants.get(n)) {
            Some((req, resp)) => Cow::Owned(Self { request: req.clone(), response: resp.clone(), ..self.clone() }),
            None => Cow::Borrowed(self),
        }
    }

    /// The name of the binary response's placeholder, when the response is a raw body.
    pub fn binary_response(&self) -> Option<&str> {
        binary(&self.response)
    }

    pub fn decode_request(&self, body: &Value) -> Result<TypeValue, CodecError> {
        decode(&self.request, body)
            .ok_or_else(|| CodecError::decode(self.ty.as_str(), "the body doesn't have the style's shape"))
    }

    /// Decodes a provider answer in this style. `content_type` is the upstream header.
    pub fn decode_response(&self, raw: &[u8], content_type: Option<&str>) -> Result<TypeValue, CodecError> {
        if let Some(name) = self.binary_response() {
            return Ok(binary_value(name, raw, content_type));
        }
        let v: Value = serde_json::from_slice(raw).map_err(|e| CodecError::decode("response", e.to_string()))?;
        decode(&self.response, &v)
            .ok_or_else(|| CodecError::decode("response", "the answer doesn't have the wire's shape"))
    }

    pub fn encode_request(&self, ir: &TypeValue, ctx: &dyn Lookup) -> Value {
        encode(&self.request, ir, ctx)
    }

    /// The client body: JSON, or raw bytes for a binary response. Returns the bytes and
    /// their content type.
    pub fn encode_response(&self, ir: &TypeValue, ctx: &dyn Lookup) -> (Vec<u8>, String) {
        if let Some(name) = self.binary_response() {
            let bytes = ir.get(name).and_then(|v| crate::primitives::body::bytes_of(&v)).unwrap_or_default();
            let ctype = ir.str("output.mime_type").unwrap_or_else(|| "application/octet-stream".into());
            return (bytes, ctype);
        }
        let mut ir = ir.clone();
        if let Some(form) = self.vector {
            for item in &mut ir.items {
                if let Some(v) = item.0.get_mut("output.embedding") {
                    *v = embeddings::convert(form, v);
                }
            }
        }
        (encode(&self.response, &ir, ctx).to_string().into_bytes(), "application/json".into())
    }

    /// A job status body in this style.
    pub fn encode_job(&self, job: &Bindings, status: JobStatus) -> Option<Value> {
        let j = self.job.as_ref()?;
        let mut b = job.clone();
        b.set("job.status", j.status_map.get(status.as_str()).cloned().unwrap_or_else(|| status.as_str().to_owned()));
        b.set("job.done", status.done());
        let mut v = template::render(&j.status, &b);
        prune(&mut v);
        Some(v)
    }

    /// Reads a job status body in this style: the bindings and the IR status.
    pub fn decode_job(&self, v: &Value) -> Option<(Bindings, JobStatus)> {
        let j = self.job.as_ref()?;
        let b = template::match_value(&lenient(&j.status), v)?;
        let raw = b.str("job.status").unwrap_or_default();
        let ir = j.status_map.iter().find(|(_, s)| s.as_str() == raw).map(|(ir, _)| ir.as_str()).unwrap_or(raw);
        Some((b.clone(), job_status(ir, b.get("job.done").and_then(Value::as_bool))))
    }
}

/// Removes empty objects and arrays, bottom up: a Gemini operation with no error has no
/// `error` key at all.
fn prune(v: &mut Value) {
    let empty =
        |v: &Value| matches!(v, Value::Object(o) if o.is_empty()) || matches!(v, Value::Array(a) if a.is_empty());
    match v {
        Value::Object(o) => {
            o.values_mut().for_each(prune);
            o.retain(|_, v| !empty(v));
        }
        Value::Array(a) => {
            a.iter_mut().for_each(prune);
            a.retain(|v| !empty(v));
        }
        _ => {}
    }
}

/// The IR state for a status word: the style's own map first, then the common words.
pub fn job_status(word: &str, done: Option<bool>) -> JobStatus {
    match word.to_ascii_lowercase().as_str() {
        "queued" | "pending" | "submitted" => JobStatus::Queued,
        "completed" | "succeeded" | "success" | "done" => JobStatus::Completed,
        "failed" | "cancelled" | "canceled" | "expired" | "error" => JobStatus::Failed,
        _ if done == Some(true) => JobStatus::Completed,
        _ => JobStatus::InProgress,
    }
}

/// Decodes an inline endpoint's response mapping (IR field → path). A path with `[*]`
/// yields items.
pub fn decode_mapped(
    map: &[(String, FieldPath)],
    raw: &[u8],
    content_type: Option<&str>,
) -> Result<TypeValue, CodecError> {
    if map.is_empty() {
        return Ok(binary_value("output.audio", raw, content_type));
    }
    let v: Value = serde_json::from_slice(raw).map_err(|e| CodecError::decode("response", e.to_string()))?;
    let mut out = TypeValue::default();
    for (field, path) in map {
        let name = format!("output.{field}");
        if path.has_each() {
            let vals = template::select(path, &v);
            out.items.resize_with(out.items.len().max(vals.len()), Bindings::new);
            for (item, val) in out.items.iter_mut().zip(vals) {
                item.set(&name, val.clone());
            }
        } else if let Some(val) = template::select_one(path, &v) {
            out.scalars.set(&name, val.clone());
        }
    }
    Ok(out)
}

fn binary_value(name: &str, raw: &[u8], content_type: Option<&str>) -> TypeValue {
    let mut scalars = Bindings::new().with(name, STANDARD.encode(raw));
    if let Some(c) = content_type {
        scalars.set("output.mime_type", c);
    }
    TypeValue { scalars, items: Vec::new() }
}

fn binary(t: &Template) -> Option<&str> {
    match t {
        Template::Hole { name, .. } if name.starts_with("output.") => Some(name),
        _ => None,
    }
}

/// `t` with literal leaves turned into optional holes nobody reads.
fn lenient(t: &Template) -> Template {
    match t {
        Template::Null | Template::Bool(_) | Template::Int(_) | Template::Float(_) | Template::Str(_) => {
            Template::Hole { name: "_".into(), optional: true }
        }
        Template::Array(items) => Template::Array(items.iter().map(lenient).collect()),
        Template::Object(fields) => Template::Object(fields.iter().map(|(k, v)| (k.clone(), lenient(v))).collect()),
        other => other.clone(),
    }
}

/// Names bound inside a "for each" array.
fn item_names(t: &Template, inside: bool, out: &mut BTreeSet<String>) {
    match t {
        Template::Hole { name, .. } if inside => {
            out.insert(name.clone());
        }
        Template::Interp(_) if inside => out.extend(t.holes().into_iter().map(str::to_owned)),
        Template::Array(items) => {
            let each = items.len() == 1;
            items.iter().for_each(|i| item_names(i, inside || each, out));
        }
        Template::Object(fields) => fields.iter().for_each(|(_, v)| item_names(v, inside, out)),
        _ => {}
    }
}

pub fn decode(t: &Template, v: &Value) -> Option<TypeValue> {
    let all = template::match_all(&lenient(t), v);
    let first = all.first()?;
    let mut per_item = BTreeSet::new();
    item_names(t, false, &mut per_item);
    per_item.insert("_".into());
    let mut scalars = first.clone();
    scalars.0.retain(|k, _| !per_item.contains(k));
    let items: Vec<Bindings> = all
        .into_iter()
        .map(|mut b| {
            b.0.retain(|k, _| per_item.contains(k) && k != "_");
            b
        })
        .filter(|b| !b.0.is_empty())
        .collect();
    Some(TypeValue { scalars, items })
}

pub fn encode(t: &Template, ir: &TypeValue, ctx: &dyn Lookup) -> Value {
    render(t, ir, None, ctx).unwrap_or(Value::Null)
}

/// Lookup order: the current item (with its ordinal as `output.index`), the IR, then `ctx`.
struct Scope<'a> {
    item: Option<(usize, &'a Bindings)>,
    ir: &'a TypeValue,
    ctx: &'a dyn Lookup,
}

impl Lookup for Scope<'_> {
    fn lookup(&self, name: &str) -> Option<Cow<'_, Value>> {
        if let Some((i, item)) = self.item {
            if let Some(v) = item.get(name) {
                return Some(Cow::Borrowed(v));
            }
            if name == "output.index" {
                return Some(Cow::Owned(Value::from(i)));
            }
            // Never another item's value.
            return self.ir.scalars.get(name).map(Cow::Borrowed).or_else(|| self.ctx.lookup(name));
        }
        self.ir.get(name).or_else(|| self.ctx.lookup(name))
    }
}

fn render(t: &Template, ir: &TypeValue, item: Option<(usize, &Bindings)>, ctx: &dyn Lookup) -> Option<Value> {
    let scope = Scope { item, ir, ctx };
    match t {
        Template::Array(items) if items.len() == 1 => {
            let each = &items[0];
            let names = each.holes();
            let per_item = ir.items.iter().any(|b| names.iter().any(|n| b.get(n).is_some()));
            let out: Vec<Value> = if item.is_none() && per_item {
                ir.items.iter().enumerate().filter_map(|(i, b)| render(each, ir, Some((i, b)), ctx)).collect()
            } else if let Some(list) = spread(each, &scope) {
                list.iter().enumerate().filter_map(|(i, b)| render(each, ir, Some((i, b)), ctx)).collect()
            } else {
                render(each, ir, item, ctx).into_iter().collect()
            };
            Some(Value::Array(out))
        }
        Template::Array(items) => Some(Value::Array(items.iter().filter_map(|i| render(i, ir, item, ctx)).collect())),
        Template::Object(fields) => Some(Value::Object(
            fields.iter().filter_map(|(k, v)| Some((k.clone(), render(v, ir, item, ctx)?))).collect(),
        )),
        leaf => {
            let v = template::render(&Template::Array(vec![leaf.clone()]), &scope);
            v.as_array().and_then(|a| a.first().cloned())
        }
    }
}

/// Outside any item, a "for each" naming exactly one scalar array iterates it: an OpenAI
/// `input` list into Gemini's `requests`.
fn spread(each: &Template, scope: &Scope<'_>) -> Option<Vec<Bindings>> {
    if scope.item.is_some() {
        return None;
    }
    let lists: Vec<(&str, &Vec<Value>)> = each
        .holes()
        .into_iter()
        .filter_map(|n| scope.ir.scalars.get(n).and_then(Value::as_array).map(|a| (n, a)))
        .collect();
    let [(name, list)] = lists.as_slice() else { return None };
    Some(list.iter().map(|v| Bindings::new().with(name, v.clone())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t(s: &str) -> Template {
        Template::parse(&toml::from_str::<toml::Table>(&format!("t = {s}")).unwrap()["t"]).unwrap()
    }

    const OPENAI_EMB: &str = r#"{ object = "list", data = [{ object = "embedding", index = "{output.index}", embedding = "{output.embedding}" }], model = "{response.model}", usage = { prompt_tokens = "{usage.input?}", total_tokens = "{usage.total?}" } }"#;
    const GEMINI_BATCH: &str = r#"{ embeddings = [{ values = "{output.embedding}" }] }"#;

    #[test]
    fn items_cross_styles_and_get_their_ordinal() {
        let g = decode(&t(GEMINI_BATCH), &json!({"embeddings": [{"values": [0.1]}, {"values": [0.2]}]})).unwrap();
        assert_eq!(g.items.len(), 2);
        let out = encode(&t(OPENAI_EMB), &g, &Bindings::new().with("response.model", "m"));
        assert_eq!(out["data"][1], json!({"object": "embedding", "index": 1, "embedding": [0.2]}));
        assert_eq!(out["model"], "m");
        assert!(out["usage"].as_object().unwrap().is_empty(), "optional usage omitted");
    }

    #[test]
    fn decoding_ignores_literals_and_keeps_usage_scalar() {
        let v = decode(
            &t(OPENAI_EMB),
            &json!({"data": [{"embedding": [1.0], "index": 0}], "model": "x", "usage": {"prompt_tokens": 3}}),
        )
        .unwrap();
        assert_eq!(v.usage("input"), Some(3));
        assert_eq!(v.items[0].get("output.embedding"), Some(&json!([1.0])));
    }

    #[test]
    fn a_scalar_list_spreads_into_a_for_each() {
        let openai_req = t(r#"{ model = "{model}", input = "{input.text}" }"#);
        let ir = decode(&openai_req, &json!({"model": "u", "input": ["a", "b"]})).unwrap();
        let gemini = t(
            r#"{ requests = [{ model = "models/{model.upstream_id}", content = { parts = [{ text = "{input.text}" }] } }] }"#,
        );
        let out = encode(&gemini, &ir, &Bindings::new().with("model.upstream_id", "e1"));
        assert_eq!(out["requests"][1], json!({"model": "models/e1", "content": {"parts": [{"text": "b"}]}}));
        let single = decode(&openai_req, &json!({"model": "u", "input": "a"})).unwrap();
        assert_eq!(encode(&gemini, &single, &Bindings::new())["requests"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn one_item_fills_a_scalar_placeholder() {
        let gemini_one = t(r#"{ contents = [{ parts = [{ text = "{input.prompt}" }] }] }"#);
        let ir = decode(&gemini_one, &json!({"contents": [{"parts": [{"text": "a cat"}]}]})).unwrap();
        assert_eq!(encode(&t(r#"{ prompt = "{input.prompt}" }"#), &ir, &Bindings::new()), json!({"prompt": "a cat"}));
    }

    #[test]
    fn binary_and_mapped_responses() {
        let d = TypeDecl {
            encoding: BodyEncoding::Json,
            request: toml::Value::Table(Default::default()),
            response: toml::Value::String("{output.audio}".into()),
            vector: None,
            job: None,
            usage: None,
            variants: Default::default(),
        };
        let c = TypeCodec::compile(ModelType::Tts, &d).unwrap();
        let ir = c.decode_response(b"ID3", Some("audio/mpeg")).unwrap();
        assert_eq!(c.encode_response(&ir, &Bindings::new()), (b"ID3".to_vec(), "audio/mpeg".into()));

        let map = vec![
            ("text".to_owned(), FieldPath::parse("text").unwrap()),
            ("language".to_owned(), FieldPath::parse("language_code").unwrap()),
        ];
        let v = decode_mapped(&map, br#"{"text": "hi", "language_code": "en"}"#, None).unwrap();
        assert_eq!(v.str("output.text").as_deref(), Some("hi"));
        assert_eq!(v.str("output.language").as_deref(), Some("en"));
    }

    #[test]
    fn job_status_words_map_to_the_ir() {
        assert_eq!(job_status("pending", None), JobStatus::Queued);
        assert_eq!(job_status("expired", None), JobStatus::Failed);
        assert_eq!(job_status("whatever", Some(true)), JobStatus::Completed);
        assert_eq!(job_status("RUNNING", Some(false)), JobStatus::InProgress);
    }
}
