//! Model list routes in each style's shape (T114, research R15).
//!
//! Listed: every unified model, and every direct `provider/model` of every type on a
//! provider the operator can reach (an enabled account, or no auth needed) through an
//! endpoint of that type. The OpenAI shape is the default; `anthropic-messages` and
//! `gemini` answer in theirs. Each entry carries its type in `nullrouter.type`.

use std::collections::BTreeSet;

use axum::response::Response;
use nullrouter_engine::state::EngineState;
use nullrouter_registry::schema::ModelType;
use serde_json::{Value, json};

use crate::relay;
use crate::router::Matched;

/// One listed model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub id: String,
    pub display_name: String,
    pub owned_by: String,
    pub ty: ModelType,
}

/// Every model a client can reach, unified models first, in load order.
pub fn listed(st: &EngineState) -> Vec<Listed> {
    let reg = &st.registry;
    let mut out: Vec<Listed> = reg
        .unified_models()
        .map(|u| Listed {
            id: u.name.clone(),
            display_name: u.name.clone(),
            owned_by: "0router".into(),
            ty: u.kind.and_then(ModelType::from_capability).unwrap_or(ModelType::Text),
        })
        .collect();
    let mut seen: BTreeSet<String> = out.iter().map(|l| l.id.clone()).collect();
    for p in reg.providers() {
        let no_auth = p.auth.as_ref().is_some_and(|a| a.no_auth);
        if !no_auth && st.accounts.for_provider(&p.id).next().is_none() {
            continue;
        }
        for s in ModelType::ALLOWED {
            let Some(ty) = ModelType::parse(s) else { continue };
            if reg.endpoints(&p.id, ty).is_empty() {
                continue;
            }
            for m in reg.catalog(&p.id, ty.capability()).unwrap_or_default() {
                let id = format!("{}/{}", p.id, m.id());
                if !seen.insert(id.clone()) {
                    continue;
                }
                let display_name = reg.model(&p.id, m.id()).map_or_else(|_| m.id().to_owned(), |i| i.name.into_owned());
                out.push(Listed { id, display_name, owned_by: p.id.clone(), ty });
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    OpenAi,
    Anthropic,
    Gemini,
}

fn shape(m: &Matched<'_>) -> Shape {
    match m.entry.style.file.id.as_str() {
        "anthropic-messages" => Shape::Anthropic,
        "gemini" => Shape::Gemini,
        _ => Shape::OpenAi,
    }
}

/// What a Gemini client may call on a model of `ty`.
fn gemini_methods(ty: ModelType) -> &'static [&'static str] {
    match ty {
        ModelType::Text => &["generateContent", "streamGenerateContent", "countTokens"],
        ModelType::Embeddings => &["embedContent", "batchEmbedContents"],
        ModelType::Image | ModelType::Tts => &["generateContent"],
        ModelType::Video => &["predictLongRunning"],
        _ => &[],
    }
}

fn entry(shape: Shape, l: &Listed) -> Value {
    let nr = json!({ "type": l.ty.as_str() });
    match shape {
        Shape::OpenAi => {
            json!({ "id": l.id, "object": "model", "created": 0, "owned_by": l.owned_by, "nullrouter": nr })
        }
        Shape::Anthropic => json!({
            "id": l.id,
            "type": "model",
            "display_name": l.display_name,
            "created_at": "1970-01-01T00:00:00Z",
            "nullrouter": nr,
        }),
        Shape::Gemini => json!({
            "name": format!("models/{}", l.id),
            "displayName": l.display_name,
            "supportedGenerationMethods": gemini_methods(l.ty),
            "nullrouter": nr,
        }),
    }
}

/// `list_models`: every reachable model in the route's style shape.
pub fn list(st: &EngineState, m: &Matched<'_>, id: &str) -> Response {
    let shape = shape(m);
    let data: Vec<Value> = listed(st).iter().map(|l| entry(shape, l)).collect();
    let body = match shape {
        Shape::OpenAi => json!({ "object": "list", "data": data }),
        Shape::Anthropic => json!({
            "data": data,
            "has_more": false,
            "first_id": data.first().map(|e| e["id"].clone()),
            "last_id": data.last().map(|e| e["id"].clone()),
        }),
        Shape::Gemini => json!({ "models": data }),
    };
    relay::json(200, &body, id)
}

/// `get_model`: one reachable model, or 404 in the style's error shape.
pub fn get(st: &EngineState, m: &Matched<'_>, model: &str, id: &str) -> Response {
    let model = model.strip_prefix("models/").unwrap_or(model);
    match listed(st).iter().find(|l| l.id == model) {
        Some(l) => relay::json(200, &entry(shape(m), l), id),
        None => crate::serve::style_error(m, 404, &format!("0router: no reachable model {model}"), id),
    }
}
