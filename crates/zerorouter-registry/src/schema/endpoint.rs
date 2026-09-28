//! Schema-2 upstream endpoints (contracts/provider-schema-v2.md § Endpoints).

use std::collections::BTreeMap;
use std::fmt;

use indexmap::IndexMap;
use serde::de::value::{MapAccessDeserializer, SeqAccessDeserializer};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::enums::AuthScheme;
use super::primitives::{BodyEncoding, ContinuationMethod, ContinuationUnless};
use super::style::MatchRule;

/// One upstream endpoint for one model type.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub url: String,
    #[serde(default = "post")]
    pub method: String,
    /// A loaded style id. Exclusive with `body` + `response`.
    pub wire: Option<String>,
    /// Inline request template (non-text endpoints without a wire).
    pub body: Option<toml::Value>,
    /// Inline response mapping: IR field → response field path.
    pub response: Option<toml::Value>,
    /// Static, non-secret headers.
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    /// Where the core writes the secret for this endpoint, when not the provider's `[auth]`.
    pub auth: Option<EndpointAuth>,
    pub encoding: Option<BodyEncoding>,
    /// Time to response headers.
    pub timeout_ms: Option<u64>,
    pub stall_timeout_ms: Option<u64>,
    #[serde(default)]
    pub force_stream: bool,
    #[serde(default)]
    pub vision: bool,
    /// HTTP status → retry override (research R7).
    #[serde(default)]
    pub retry: BTreeMap<String, RetryOverride>,
    /// Restricts this endpoint to these model ids; empty = every model of the type.
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub voices: Vec<String>,
    #[serde(default)]
    pub errors: ErrorRules,
    pub token_count: Option<TokenCount>,
    pub continuation: Option<Continuation>,
}

/// A per-endpoint auth placement (a gateway whose Anthropic route takes `x-api-key`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointAuth {
    pub header: String,
    pub scheme: AuthScheme,
}

fn post() -> String {
    "POST".into()
}

impl Endpoint {
    /// Whether this endpoint serves `model` (by upstream id).
    pub fn serves(&self, model: &str) -> bool {
        self.models.is_empty() || self.models.iter().any(|m| m == model)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryOverride {
    pub retries: u32,
    #[serde(default)]
    pub delay_ms: u64,
}

/// In-band error rules: a 200 body or a stream event that is really an error.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorRules {
    #[serde(default)]
    pub body: Vec<ErrorRule>,
    #[serde(default)]
    pub stream: Vec<ErrorRule>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorRule {
    pub when: Option<MatchRule>,
    /// SSE event name that marks an error (stream rules).
    pub event: Option<String>,
    /// Field path of the message.
    pub message: String,
    /// Field path of an HTTP-like status.
    pub status: Option<String>,
    /// Error type value → HTTP status.
    #[serde(default)]
    pub status_map: BTreeMap<String, u16>,
    /// Fixed HTTP status when the body carries none.
    pub code: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenCount {
    pub url: String,
}

/// How a cut answer is continued (research R9).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Continuation {
    pub method: ContinuationMethod,
    #[serde(default)]
    pub trim_trailing_whitespace: bool,
    #[serde(default)]
    pub unless: Vec<ContinuationUnless>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub except_models: Vec<String>,
}

impl Continuation {
    pub fn applies_to(&self, model: &str) -> bool {
        let listed = |l: &[String]| l.iter().any(|m| m == model);
        (self.models.is_empty() || listed(&self.models)) && !listed(&self.except_models)
    }
}

/// `endpoints.<type>`: one table or an array of tables (one entry per wire).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Endpoints(pub Vec<Endpoint>);

impl<'de> Deserialize<'de> for Endpoints {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Endpoints;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an endpoint table or an array of endpoint tables")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Endpoints, A::Error> {
                Endpoint::deserialize(MapAccessDeserializer::new(map)).map(|e| Endpoints(vec![e]))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Endpoints, A::Error> {
                Vec::<Endpoint>::deserialize(SeqAccessDeserializer::new(seq)).map(Endpoints)
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ModelType;

    #[derive(Deserialize)]
    struct Doc {
        endpoints: BTreeMap<ModelType, Endpoints>,
    }

    #[test]
    fn one_or_many() {
        let d: Doc = toml::from_str(
            r#"
[endpoints.text]
url = "https://api.anthropic.com/v1/messages"
wire = "anthropic-messages"
retry = { 429 = { retries = 1, delay_ms = 2000 } }
continuation = { method = "assistant_prefill", unless = ["thinking_enabled"], except_models = ["x"] }

[[endpoints.embeddings]]
url = "https://a.example/v1/embeddings"
wire = "openai-chat"
[[endpoints.embeddings]]
url = "https://a.example/v1beta/models/{model}:embedContent"
wire = "gemini"
"#,
        )
        .unwrap();
        let text = &d.endpoints[&ModelType::Text].0[0];
        assert_eq!(text.method, "POST");
        assert_eq!(text.retry["429"], RetryOverride { retries: 1, delay_ms: 2000 });
        let c = text.continuation.as_ref().unwrap();
        assert!(c.applies_to("y") && !c.applies_to("x"));
        assert_eq!(d.endpoints[&ModelType::Embeddings].0.len(), 2);
    }

    #[test]
    fn unknown_key_is_named() {
        let err = toml::from_str::<Doc>("[endpoints.text]\nurl = \"https://a.example\"\nquirks = []\n")
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("unknown field `quirks`"), "{err}");
    }
}
