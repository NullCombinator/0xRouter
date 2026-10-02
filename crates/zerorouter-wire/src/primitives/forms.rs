//! Thinking and response-format forms: one IR value, several wire shapes.
//!
//! Effort ↔ budget uses 9router's web-standard maps (`translator/concerns/thinking.js`).

use serde_json::{Map, Value, json};
use zerorouter_registry::schema::{ResponseFormatForm, ThinkingForm};

use crate::ir::{ResponseFormat, Thinking};

const LEVEL_TO_BUDGET: &[(&str, u64)] =
    &[("minimal", 512), ("low", 1024), ("medium", 8192), ("high", 24576), ("xhigh", 32768), ("max", 128000)];

pub fn effort_to_budget(effort: &str) -> Option<u64> {
    let e = effort.to_ascii_lowercase();
    LEVEL_TO_BUDGET.iter().find(|(l, _)| *l == e).map(|(_, b)| *b)
}

pub fn budget_to_effort(budget: u64) -> Option<&'static str> {
    Some(match budget {
        0 => return None,
        1..=768 => "minimal",
        769..=4096 => "low",
        4097..=16384 => "medium",
        16385..=28672 => "high",
        28673..=80384 => "xhigh",
        _ => "max",
    })
}

fn budget(t: &Thinking) -> u64 {
    t.budget_tokens.or_else(|| t.effort.as_deref().and_then(effort_to_budget)).unwrap_or(8192)
}

pub fn decode_thinking(form: ThinkingForm, v: &Value) -> Option<Thinking> {
    match form {
        ThinkingForm::BudgetTokens => {
            let budget_tokens = v.get("budget_tokens").and_then(Value::as_u64);
            let enabled = match v.get("type").and_then(Value::as_str) {
                Some("disabled") => false,
                Some(_) => true,
                None => budget_tokens.is_some(),
            };
            Some(Thinking { enabled, budget_tokens, effort: None })
        }
        ThinkingForm::Effort => {
            let e = v.as_str()?;
            let off = matches!(e, "none" | "off");
            Some(Thinking { enabled: !off, budget_tokens: None, effort: (!off).then(|| e.to_owned()) })
        }
        ThinkingForm::GeminiThinkingConfig => {
            let budget_tokens = v.get("thinkingBudget").and_then(Value::as_i64);
            let effort = v.get("thinkingLevel").and_then(Value::as_str).map(str::to_owned);
            Some(Thinking {
                enabled: budget_tokens != Some(0),
                budget_tokens: budget_tokens.and_then(|b| u64::try_from(b).ok()).filter(|b| *b > 0),
                effort,
            })
        }
    }
}

/// The wire value, or `None` to leave the field out.
pub fn encode_thinking(form: ThinkingForm, t: &Thinking) -> Option<Value> {
    match form {
        ThinkingForm::BudgetTokens if t.enabled => Some(json!({ "type": "enabled", "budget_tokens": budget(t) })),
        ThinkingForm::BudgetTokens => Some(json!({ "type": "disabled" })),
        ThinkingForm::Effort if t.enabled => {
            let e = t.effort.clone().or_else(|| t.budget_tokens.and_then(budget_to_effort).map(str::to_owned));
            Some(Value::String(e.unwrap_or_else(|| "medium".into())))
        }
        ThinkingForm::Effort => None,
        ThinkingForm::GeminiThinkingConfig if t.enabled => {
            Some(json!({ "thinkingBudget": budget(t), "includeThoughts": true }))
        }
        ThinkingForm::GeminiThinkingConfig => Some(json!({ "thinkingBudget": 0 })),
    }
}

pub fn decode_response_format(form: ResponseFormatForm, v: &Value) -> Option<ResponseFormat> {
    let schema_of = |o: &Value| ResponseFormat::JsonSchema {
        name: o.get("name").and_then(Value::as_str).map(str::to_owned),
        schema: o.get("schema").cloned().unwrap_or(Value::Null),
        strict: o.get("strict").and_then(Value::as_bool),
    };
    match form {
        ResponseFormatForm::ChatResponseFormat | ResponseFormatForm::ResponsesTextFormat => {
            Some(match v.get("type")?.as_str()? {
                "text" => ResponseFormat::Text,
                "json_object" => ResponseFormat::JsonObject,
                "json_schema" if form == ResponseFormatForm::ChatResponseFormat => schema_of(v.get("json_schema")?),
                "json_schema" => schema_of(v),
                _ => return None,
            })
        }
        ResponseFormatForm::GeminiResponseSchema => {
            let mime = v.get("responseMimeType")?.as_str()?;
            let schema = v.get("responseSchema").or_else(|| v.get("responseJsonSchema"));
            Some(match (mime, schema) {
                ("application/json", Some(s)) => {
                    ResponseFormat::JsonSchema { name: None, schema: s.clone(), strict: None }
                }
                ("application/json", None) => ResponseFormat::JsonObject,
                _ => ResponseFormat::Text,
            })
        }
    }
}

/// The wire value. For the gemini form it is an object merged into the field's object.
pub fn encode_response_format(form: ResponseFormatForm, rf: &ResponseFormat) -> Value {
    let schema_body = |name: &Option<String>, schema: &Value, strict: &Option<bool>| {
        let mut o = Map::new();
        if let Some(n) = name {
            o.insert("name".into(), n.clone().into());
        }
        o.insert("schema".into(), schema.clone());
        if let Some(s) = strict {
            o.insert("strict".into(), (*s).into());
        }
        o
    };
    match (form, rf) {
        (ResponseFormatForm::GeminiResponseSchema, ResponseFormat::Text) => json!({ "responseMimeType": "text/plain" }),
        (ResponseFormatForm::GeminiResponseSchema, ResponseFormat::JsonObject) => {
            json!({ "responseMimeType": "application/json" })
        }
        (ResponseFormatForm::GeminiResponseSchema, ResponseFormat::JsonSchema { schema, .. }) => {
            json!({ "responseMimeType": "application/json", "responseSchema": schema })
        }
        (_, ResponseFormat::Text) => json!({ "type": "text" }),
        (_, ResponseFormat::JsonObject) => json!({ "type": "json_object" }),
        (ResponseFormatForm::ChatResponseFormat, ResponseFormat::JsonSchema { name, schema, strict }) => {
            json!({ "type": "json_schema", "json_schema": schema_body(name, schema, strict) })
        }
        (_, ResponseFormat::JsonSchema { name, schema, strict }) => {
            let mut o = schema_body(name, schema, strict);
            o.insert("type".into(), "json_schema".into());
            Value::Object(o)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_forms_carry_budget_and_effort() {
        let t = Thinking { enabled: true, budget_tokens: Some(2000), effort: None };
        assert_eq!(encode_thinking(ThinkingForm::Effort, &t), Some(json!("low")));
        let t = Thinking { enabled: true, budget_tokens: None, effort: Some("high".into()) };
        assert_eq!(
            encode_thinking(ThinkingForm::BudgetTokens, &t),
            Some(json!({ "type": "enabled", "budget_tokens": 24576 }))
        );
        for form in [ThinkingForm::BudgetTokens, ThinkingForm::GeminiThinkingConfig] {
            let t = Thinking { enabled: true, budget_tokens: Some(4000), effort: None };
            assert_eq!(decode_thinking(form, &encode_thinking(form, &t).unwrap()), Some(t));
        }
        assert_eq!(decode_thinking(ThinkingForm::Effort, &json!("none")).map(|t| t.enabled), Some(false));
    }

    #[test]
    fn response_formats_round_trip() {
        let rf = ResponseFormat::JsonSchema {
            name: Some("r".into()),
            schema: json!({ "type": "object" }),
            strict: Some(true),
        };
        for form in [ResponseFormatForm::ChatResponseFormat, ResponseFormatForm::ResponsesTextFormat] {
            for rf in [&rf, &ResponseFormat::JsonObject, &ResponseFormat::Text] {
                assert_eq!(decode_response_format(form, &encode_response_format(form, rf)).as_ref(), Some(rf));
            }
        }
        let g = encode_response_format(ResponseFormatForm::GeminiResponseSchema, &rf);
        assert!(matches!(
            decode_response_format(ResponseFormatForm::GeminiResponseSchema, &g),
            Some(ResponseFormat::JsonSchema { name: None, .. })
        ));
    }
}
