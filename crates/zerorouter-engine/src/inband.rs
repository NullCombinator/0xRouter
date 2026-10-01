//! In-band errors (FR-024): a 200 body or a stream event that is really an error, as the
//! endpoint's `errors.body` and `errors.stream` rules declare it.
//!
//! A rule's status comes from its `status` path (a number, or a value looked up in
//! `status_map`), else from `status_map` on the error's type (`error.type`, then `type`),
//! else from its fixed `code`, else 502.

use serde_json::Value;
use zerorouter_registry::schema::ErrorRule;
use zerorouter_registry::template::FieldPath;
use zerorouter_wire::template::{body_rule_holds, select_one};

/// Status for a matched rule that names none.
const DEFAULT_STATUS: u16 = 502;

/// A matched rule's reading of the error.
#[derive(Debug, Clone, PartialEq)]
pub struct InBand {
    pub status: u16,
    pub message: String,
}

fn at<'v>(path: &str, v: &'v Value) -> Option<&'v Value> {
    FieldPath::parse(path).ok().and_then(|p| select_one(&p, v)).filter(|v| !v.is_null())
}

fn text(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_owned)
}

fn read(rule: &ErrorRule, v: &Value) -> InBand {
    let mapped = |key: &Value| rule.status_map.get(&text(key)).copied();
    let from_path = rule.status.as_deref().and_then(|p| at(p, v)).and_then(|s| {
        s.as_u64()
            .or_else(|| s.as_str().and_then(|t| t.parse().ok()))
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| (100..=599).contains(n))
            .or_else(|| mapped(s))
    });
    let from_type = || ["error.type", "type"].iter().find_map(|p| at(p, v).and_then(mapped));
    let status = from_path.or_else(from_type).or(rule.code).unwrap_or(DEFAULT_STATUS);
    let message = at(&rule.message, v).map_or_else(|| "the provider answered with an error".to_owned(), text);
    InBand { status, message }
}

/// The first `rules` entry that reads `body` (a 2xx JSON body) as an error. A rule with no
/// `when` matches when its message path is present.
pub fn body(rules: &[ErrorRule], body: &Value) -> Option<InBand> {
    rules
        .iter()
        .find(|r| match &r.when {
            Some(w) => body_rule_holds(w, body),
            None => at(&r.message, body).is_some(),
        })
        .map(|r| read(r, body))
}

/// The first `rules` entry that reads a stream frame as an error: its `event` names the
/// frame's event, and its `when` holds for the frame's data. A rule with neither matches
/// when its message path is present.
pub fn frame(rules: &[ErrorRule], event: Option<&str>, data: &str) -> Option<InBand> {
    if rules.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(data).ok()?;
    rules
        .iter()
        .find(|r| {
            let named = r.event.as_deref().is_none_or(|e| event == Some(e));
            let holds = match &r.when {
                Some(w) => body_rule_holds(w, &v),
                None => r.event.is_some() || at(&r.message, &v).is_some(),
            };
            named && holds
        })
        .map(|r| read(r, &v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(toml: &str) -> Vec<ErrorRule> {
        #[derive(serde::Deserialize)]
        struct R {
            r: Vec<ErrorRule>,
        }
        toml::from_str::<R>(toml).unwrap().r
    }

    #[test]
    fn a_body_rule_reads_status_and_message() {
        let r =
            rules(r#"r = [{ when = { path_present = "error" }, message = "error.message", status = "error.code" }]"#);
        let got = body(&r, &json!({"error": {"message": "quota", "code": 429}})).unwrap();
        assert_eq!(got, InBand { status: 429, message: "quota".into() });
        assert!(body(&r, &json!({"choices": []})).is_none());
        let got = body(&r, &json!({"error": {"message": "x", "code": "429"}})).unwrap();
        assert_eq!(got.status, 429, "a numeric string");
        let got = body(&r, &json!({"error": {"message": "x"}})).unwrap();
        assert_eq!(got.status, DEFAULT_STATUS);
    }

    #[test]
    fn a_stream_rule_maps_the_error_type() {
        let r =
            rules(r#"r = [{ event = "error", message = "error.message", status_map = { overloaded_error = 529 } }]"#);
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(frame(&r, Some("error"), data), Some(InBand { status: 529, message: "Overloaded".into() }));
        assert!(frame(&r, Some("message_delta"), data).is_none());
        assert!(frame(&r, Some("error"), "not json").is_none());
    }

    #[test]
    fn a_fixed_code_applies_when_nothing_maps() {
        let r = rules(r#"r = [{ when = { path_equals = ["status", "failed"] }, message = "detail", code = 503 }]"#);
        assert_eq!(frame(&r, None, r#"{"status":"failed","detail":"boom"}"#).unwrap().status, 503);
        assert!(frame(&r, None, r#"{"status":"ok"}"#).is_none());
    }
}
