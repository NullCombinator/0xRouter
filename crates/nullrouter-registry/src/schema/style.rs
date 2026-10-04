//! API-style files (`styles/bundled/*.toml`, contracts/api-style-schema.md).
//!
//! A style is both a client front door (routes, key and session carriers) and an upstream
//! wire that provider endpoints name. Template-valued fields stay `toml::Value` here; the
//! style gate parses them with [`crate::template`] and checks each against its context's
//! placeholder set.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::primitives::{
    Alternation, ArgumentsForm, AsyncJob, AudioCollector, BlockModel, BodyEncoding, ContentForm, EmbeddingVector,
    FinishReason, Framing, InputSemantics, KeyScheme, MediaCodec, ModelType, PartKind, Repair, RouteOp,
    SessionExtractor, StreamOn, SystemLayout, TokenEstimator, ToolArgumentsMode, ToolCallsLayout, ToolResultContent,
    ToolResultMatch, ToolResultsLayout,
};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleFile {
    pub schema: i64,
    pub kind: String,
    pub id: String,
    pub access_key: AccessKey,
    #[serde(default)]
    pub session: SessionCarriers,
    #[serde(default)]
    pub routes: Vec<Route>,
    pub text: Option<TextCodec>,
    pub embeddings: Option<TypeCodec>,
    pub image: Option<TypeCodec>,
    pub tts: Option<TypeCodec>,
    pub stt: Option<TypeCodec>,
    pub video: Option<TypeCodec>,
    pub errors: ErrorShape,
    /// Where a cache marker keeps its lifetime, for styles whose markers carry one (spec 006, R3).
    pub cache_marker: Option<CacheMarker>,
    /// Rejected by the gate with a clear rule; parsed only so the message can name it.
    pub forwarding: Option<toml::Value>,
}

impl StyleFile {
    /// The codec section for `t`, if declared (text is checked separately).
    pub fn type_codec(&self, t: ModelType) -> Option<&TypeCodec> {
        match t {
            ModelType::Text => None,
            ModelType::Embeddings => self.embeddings.as_ref(),
            ModelType::Image => self.image.as_ref(),
            ModelType::Tts => self.tts.as_ref(),
            ModelType::Stt => self.stt.as_ref(),
            ModelType::Video => self.video.as_ref(),
        }
    }

    pub fn has_codec(&self, t: ModelType) -> bool {
        if t == ModelType::Text { self.text.is_some() } else { self.type_codec(t).is_some() }
    }
}

/// `[cache_marker]`: what a client's `cache_control` marker says beyond "cache here".
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheMarker {
    /// The key inside the marker that holds its lifetime (`5m`, `1h`), e.g. `ttl`.
    pub ttl: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessKey {
    pub carriers: Vec<KeyCarrier>,
}

/// Where a client puts its access key. Exactly one of `header` or `query`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyCarrier {
    pub header: Option<String>,
    pub query: Option<String>,
    #[serde(default = "raw")]
    pub scheme: KeyScheme,
}

fn raw() -> KeyScheme {
    KeyScheme::Raw
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCarriers {
    #[serde(default)]
    pub carriers: Vec<SessionCarrier>,
}

/// Where a client carries its session id. Exactly one of `header`, `path` or `extractor`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCarrier {
    pub header: Option<String>,
    /// A body field path.
    pub path: Option<String>,
    pub extractor: Option<SessionExtractor>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub method: String,
    /// `{name}` matches one segment, `{name*}` the rest including `/`.
    pub path: String,
    pub op: RouteOp,
    #[serde(rename = "type")]
    pub model_type: ModelType,
    pub model: Option<Locator>,
    pub stream: Option<StreamLocator>,
    pub discriminator: Option<MatchRule>,
    /// A named variant of the type's codec, for a route whose body differs (Gemini's
    /// `:embedContent` next to `:batchEmbedContents`).
    pub variant: Option<String>,
}

/// Where a route reads a value: a body field or a path parameter. Exactly one.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Locator {
    pub body: Option<String>,
    pub path: Option<String>,
}

/// How a route tells a streaming request: a boolean body field or a path suffix.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamLocator {
    pub body: Option<String>,
    pub path_suffix: Option<String>,
}

/// Literal equality or presence only. Every set field must hold.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchRule {
    pub header_present: Option<String>,
    pub path_present: Option<String>,
    /// `[path, value]`.
    pub path_equals: Option<Vec<String>>,
}

/// The text codec.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextCodec {
    pub layout: TextLayout,
    pub parts: BTreeMap<PartKind, PartDecl>,
    #[serde(default)]
    pub tools: Option<ToolsDecl>,
    /// IR param → field path, or `{ path, form }` for params with a named form.
    #[serde(default)]
    pub params: BTreeMap<String, ParamDecl>,
    /// Style reason → IR reason. Total over the style's reasons.
    pub finish: BTreeMap<String, FinishReason>,
    /// IR reason → style reason when encoding; defaults to the inverse of `finish` where
    /// that is unique.
    #[serde(default)]
    pub finish_out: BTreeMap<FinishReason, String>,
    pub usage: UsageDecl,
    pub response: ResponseDecl,
    pub stream: StreamDecl,
    pub count_tokens: Option<CountTokensDecl>,
    /// Cross-style repairs applied when encoding into this style from another one.
    #[serde(default)]
    pub repairs: Vec<Repair>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextLayout {
    pub system: SystemLayout,
    /// Field path of the message list (`messages`, `input`, `contents`).
    pub messages: String,
    #[serde(default = "role_field")]
    pub role: String,
    /// Field holding a message's parts (`content`, `parts`).
    pub content: String,
    #[serde(default = "always_parts")]
    pub content_form: ContentForm,
    /// IR role → style role, for roles that differ (gemini `assistant = "model"`).
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
    pub tool_calls: ToolCallsLayout,
    pub tool_results: ToolResultsLayout,
    pub arguments: ArgumentsForm,
    #[serde(default = "merge_adjacent")]
    pub alternation: Alternation,
    #[serde(default = "by_id")]
    pub tool_result_match: ToolResultMatch,
    #[serde(default = "string_or_parts")]
    pub tool_result_content: ToolResultContent,
    /// Message template for layouts whose list items are typed (Responses
    /// `{ type = "message", role, content }`). Placeholders: `{message.role}`, `{message.content}`.
    pub message: Option<toml::Value>,
}

fn role_field() -> String {
    "role".into()
}
fn always_parts() -> ContentForm {
    ContentForm::AlwaysParts
}
fn merge_adjacent() -> Alternation {
    Alternation::MergeAdjacent
}
fn by_id() -> ToolResultMatch {
    ToolResultMatch::Id
}
fn string_or_parts() -> ToolResultContent {
    ToolResultContent::StringOrParts
}

/// One IR part kind's template.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartDecl {
    pub data: toml::Value,
    /// Decode template when it differs from `data` (defaults to `data`).
    #[serde(rename = "match")]
    pub match_: Option<toml::Value>,
    /// Per-role override of `data` (Responses `output_text` for assistant text).
    #[serde(default)]
    pub roles: BTreeMap<String, toml::Value>,
    /// Media parts only: how `{media}` is encoded.
    pub codec: Option<MediaCodec>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsDecl {
    #[serde(default = "tools_path")]
    pub path: String,
    /// One tool definition. Placeholders: `{tool.name}`, `{tool.description?}`, `{tool.parameters}`.
    pub data: toml::Value,
    /// Wrapper around the whole list, when the style nests it (gemini `[{ functionDeclarations = … }]`).
    pub wrap: Option<toml::Value>,
    pub choice: Option<ToolChoiceDecl>,
}

fn tools_path() -> String {
    "tools".into()
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolChoiceDecl {
    pub path: String,
    pub auto: toml::Value,
    pub required: toml::Value,
    pub none: Option<toml::Value>,
    /// Placeholder: `{tool.name}`.
    pub named: toml::Value,
}

/// A request parameter's location, optionally with a named form.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ParamDecl {
    Path(String),
    Form {
        path: String,
        /// Checked by the gate against the param's family (thinking or response format).
        form: String,
    },
    /// A parameter the wire requires: `default` is sent when the client gave none.
    Default {
        path: String,
        default: u64,
    },
}

impl ParamDecl {
    pub fn path(&self) -> &str {
        match self {
            Self::Path(p) | Self::Form { path: p, .. } | Self::Default { path: p, .. } => p,
        }
    }
}

/// IR usage field → field path in a response body or usage-bearing event.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageDecl {
    pub input: Option<String>,
    pub output: Option<String>,
    pub cache_read: Option<String>,
    pub cache_write: Option<String>,
    pub reasoning: Option<String>,
    pub input_semantics: InputSemantics,
}

/// The non-stream response body.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseDecl {
    /// Prefix for `{response.id}` when 0router mints one (`msg_`, `chatcmpl-`, `resp_`).
    #[serde(default)]
    pub id_prefix: String,
    /// The body template. Blocks are placed by the layout.
    pub body: toml::Value,
    /// Decode template when it differs from `body` (defaults to `body`).
    #[serde(rename = "match")]
    pub match_: Option<toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamDecl {
    pub framing: Framing,
    pub blocks: BlockModel,
    pub tool_arguments: ToolArgumentsMode,
    pub events: Vec<StreamEventDecl>,
    /// Chat-completions audio modality collector (openrouter TTS fallback).
    pub audio: Option<AudioCollector>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventDecl {
    pub on: StreamOn,
    /// SSE event name, for named framings.
    pub event: Option<String>,
    pub data: toml::Value,
    /// Decode template when it differs from `data`.
    #[serde(rename = "match")]
    pub match_: Option<toml::Value>,
    /// Emit only when this request field is true (chat `stream_options.include_usage`).
    pub when_request: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountTokensDecl {
    /// Placeholder: `{count.input}`.
    pub response: toml::Value,
    #[serde(default = "estimate_9router")]
    pub estimator: TokenEstimator,
}

fn estimate_9router() -> TokenEstimator {
    TokenEstimator::Estimate9router
}

/// A non-text codec: request and response templates plus the type's primitives.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeCodec {
    #[serde(default = "json")]
    pub encoding: BodyEncoding,
    pub request: toml::Value,
    pub response: toml::Value,
    /// Embeddings only.
    pub vector: Option<EmbeddingVector>,
    /// Video only.
    pub job: Option<JobDecl>,
    pub usage: Option<UsageDecl>,
    /// Alternative request/response shapes that routes select by name.
    #[serde(default)]
    pub variants: BTreeMap<String, CodecVariant>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecVariant {
    pub request: toml::Value,
    pub response: toml::Value,
}

fn json() -> BodyEncoding {
    BodyEncoding::Json
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobDecl {
    pub mode: AsyncJob,
    /// Job status body. Placeholders: `{job.id}`, `{job.status}`, `{job.error?}`, `{job.created}`,
    /// `{job.model}`, `{job.done}` (a boolean) and `{job.content_url?}`. Empty objects are
    /// pruned after rendering.
    pub status: toml::Value,
    /// IR job status (`queued`, `in_progress`, `completed`, `failed`) → style status.
    pub status_map: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorShape {
    pub body: toml::Value,
    /// HTTP status → style error type. Keys are status codes.
    pub type_map: BTreeMap<String, String>,
    pub stream_event: StreamTemplate,
    pub keepalive: Option<StreamTemplate>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamTemplate {
    pub event: Option<String>,
    pub data: toml::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &str = r#"
schema = 1
kind = "api-style"
id = "mini"

[access_key]
carriers = [{ header = "authorization", scheme = "bearer" }]

[[routes]]
method = "POST"
path = "/v1/chat"
op = "generate"
type = "text"
model = { body = "model" }
stream = { body = "stream" }

[text.layout]
system = "first_message"
messages = "messages"
content = "content"
content_form = "string_when_text_only"
tool_calls = "message_field"
tool_results = "tool_role_message"
arguments = "json_string"

[text.parts.text]
data = { type = "text", text = "{part.text}" }

[text.params]
max_tokens = "max_tokens"
thinking = { path = "reasoning_effort", form = "effort" }

[text.finish]
stop = "stop"
length = "length"

[text.usage]
input = "usage.prompt_tokens"
output = "usage.completion_tokens"
input_semantics = "includes_cache"

[text.response]
id_prefix = "c-"
body = { id = "{response.id}" }

[text.stream]
framing = "sse_data_done"
blocks = "implicit"
tool_arguments = "fragments"

[[text.stream.events]]
on = "text_delta"
data = { delta = "{delta.text}" }

[errors]
body = { error = { message = "{error.message}" }, nullrouter = "{error.details}" }
type_map = { 400 = "bad" }
stream_event = { data = "{error.body}" }
"#;

    #[test]
    fn parses_minimal_style() {
        let s: StyleFile = toml::from_str(MINI).unwrap();
        assert_eq!(s.id, "mini");
        assert_eq!(s.routes[0].model_type, ModelType::Text);
        let text = s.text.as_ref().unwrap();
        assert_eq!(text.params["thinking"].path(), "reasoning_effort");
        assert_eq!(text.params["max_tokens"], ParamDecl::Path("max_tokens".into()));
        assert_eq!(text.stream.events[0].on, StreamOn::TextDelta);
        assert!(s.has_codec(ModelType::Text) && !s.has_codec(ModelType::Image));
    }

    #[test]
    fn rejects_unknown_keys_and_values() {
        let err = toml::from_str::<StyleFile>(
            &MINI.replace("tool_arguments = \"fragments\"", "tool_arguments = \"fragments\"\nextra = 1"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown field `extra`"), "{err}");
        let err = toml::from_str::<StyleFile>(&MINI.replace("sse_data_done", "xml")).unwrap_err().to_string();
        assert!(err.contains("unknown framing \"xml\""), "{err}");
    }
}
