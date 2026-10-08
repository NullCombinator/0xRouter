//! Schema-2 upstream endpoints (contracts/provider-schema-v2.md § Endpoints).

use std::collections::BTreeMap;
use std::fmt;

use indexmap::IndexMap;
use serde::de::value::{MapAccessDeserializer, SeqAccessDeserializer};
use serde::de::{Error as _, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::enums::{AuthScheme, closed_enum};
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
    /// Time to the TCP/TLS connection.
    pub connect_timeout_ms: Option<u64>,
    /// Time from the response headers to the first model output; 0 = off.
    pub first_token_timeout_ms: Option<u64>,
    /// Only `false` has an effect: this endpoint's host doesn't speak HTTP/2, so the provider's
    /// requests use HTTP/1.1.
    pub http2: Option<bool>,
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
    /// Request parameters set on every request to this endpoint (research R8).
    #[serde(default)]
    pub force: ForceMap,
    /// Video only: where a job is polled, with `{id}` for the provider's job id; absent,
    /// polls go to `{url}/{id}`.
    pub poll_url: Option<String>,
    /// Video only: how the provider's job bodies read, when the wire's job shape doesn't.
    pub job: Option<JobMapping>,
}

closed_enum!(
    /// The IR job states a provider's status values map to.
    JobState, "job state" {
        Queued = "queued",
        InProgress = "in_progress",
        Completed = "completed",
        Failed = "failed",
    }
);

/// `job = { … }` on a video endpoint: field paths into the submit answer and the poll
/// answer (xAI: `{request_id}`, then `{status, video: {url}, error}`). Data only: every
/// path is read, nothing is computed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobMapping {
    /// Path of the provider's job id in the submit answer.
    pub id: String,
    /// Path of the status value in a poll answer (absent in a submit answer: queued).
    pub status: String,
    /// Provider status value → IR state; values not listed read as the common words
    /// (`pending` queued, `done` completed, `expired` failed, others in progress).
    #[serde(default)]
    pub status_map: BTreeMap<String, JobState>,
    /// Path of the finished video's download URL; the content is fetched from it instead
    /// of a `{poll}/content` route.
    pub content_url: Option<String>,
    /// Path of a failed job's error message.
    pub error: Option<String>,
}

impl JobMapping {
    /// The declared paths, by key.
    pub fn paths(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [
            ("id", Some(&self.id)),
            ("status", Some(&self.status)),
            ("content_url", self.content_url.as_ref()),
            ("error", self.error.as_ref()),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k, v.as_str())))
    }
}

closed_enum!(
    /// The request parameters a plugin may force (research R8, spec Clarifications Q6).
    /// None of them is conversation content.
    ForcedParam, "forced parameter" {
        Store = "store",
        ReasoningSummary = "reasoning.summary",
        ReasoningEffort = "reasoning.effort",
        Include = "include",
    }
);

impl ForcedParam {
    /// The body path the value is written to.
    pub fn path(self) -> &'static [&'static str] {
        match self {
            Self::Store => &["store"],
            Self::ReasoningSummary => &["reasoning", "summary"],
            Self::ReasoningEffort => &["reasoning", "effort"],
            Self::Include => &["include"],
        }
    }

    /// `include` is appended to the client's list, deduplicated; the others replace.
    pub fn appends(self) -> bool {
        self == Self::Include
    }

    fn check(self, v: &toml::Value) -> Result<(), String> {
        let ok = match self {
            Self::Store => v.is_bool(),
            Self::ReasoningSummary | Self::ReasoningEffort => v.is_str(),
            Self::Include => v.as_array().is_some_and(|a| a.iter().all(toml::Value::is_str)),
        };
        let want = match self {
            Self::Store => "a boolean",
            Self::ReasoningSummary | Self::ReasoningEffort => "a string",
            Self::Include => "a list of strings",
        };
        if ok { Ok(()) } else { Err(format!("{self} must be {want}")) }
    }
}

/// `force = { … }` on an endpoint or a model: keys from [`ForcedParam`], values typed per key.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ForceMap(pub IndexMap<ForcedParam, toml::Value>);

impl ForceMap {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (ForcedParam, &toml::Value)> {
        self.0.iter().map(|(k, v)| (*k, v))
    }
}

impl<'de> Deserialize<'de> for ForceMap {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = IndexMap::<String, toml::Value>::deserialize(d)?;
        let mut out = IndexMap::new();
        for (k, v) in raw {
            let Some(p) = ForcedParam::parse(&k) else {
                return Err(D::Error::custom(format!(
                    "{k} can't be forced; allowed: {}",
                    ForcedParam::ALLOWED.join(", ")
                )));
            };
            p.check(&v).map_err(D::Error::custom)?;
            out.insert(p, v);
        }
        Ok(Self(out))
    }
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
    fn force_keys_and_values_are_closed() {
        let ok: Doc = toml::from_str(
            "[endpoints.text]\nurl = \"https://a.example\"\nforce = { store = false, \"reasoning.effort\" = \"high\", include = [\"reasoning.encrypted_content\"] }\n",
        )
        .unwrap();
        let f = &ok.endpoints[&ModelType::Text].0[0].force;
        assert_eq!(
            f.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            ForcedParam::ALL.iter().copied().filter(|p| *p != ForcedParam::ReasoningSummary).collect::<Vec<_>>()
        );
        assert_eq!(ForcedParam::ReasoningEffort.path(), ["reasoning", "effort"]);
        let err = |force: &str| {
            toml::from_str::<Doc>(&format!("[endpoints.text]\nurl = \"https://a.example\"\nforce = {force}\n"))
                .err()
                .unwrap()
                .to_string()
        };
        assert!(err("{ messages = [] }").contains("messages can't be forced"));
        assert!(err("{ store = \"no\" }").contains("store must be a boolean"));
        assert!(err("{ include = [1] }").contains("include must be a list of strings"));
    }

    #[test]
    fn a_video_endpoint_declares_its_job_shape() {
        let d: Doc = toml::from_str(
            r#"
[endpoints.video]
url = "https://api.x.ai/v1/videos/generations"
wire = "openai-chat"
poll_url = "https://api.x.ai/v1/videos/{id}"
job = { id = "request_id", status = "status", status_map = { pending = "queued", done = "completed" }, content_url = "video.url", error = "error.message" }
"#,
        )
        .unwrap();
        let e = &d.endpoints[&ModelType::Video].0[0];
        assert_eq!(e.poll_url.as_deref(), Some("https://api.x.ai/v1/videos/{id}"));
        let job = e.job.as_ref().unwrap();
        assert_eq!(job.status_map["done"], JobState::Completed);
        assert_eq!(
            job.paths().collect::<Vec<_>>(),
            [("id", "request_id"), ("status", "status"), ("content_url", "video.url"), ("error", "error.message")]
        );
        let err = toml::from_str::<Doc>(
            "[endpoints.video]\nurl = \"https://a.example\"\njob = { id = \"a\", status = \"b\", status_map = { x = \"gone\" } }\n",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(err.contains("unknown job state \"gone\"; allowed: queued, in_progress, completed, failed"), "{err}");
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
