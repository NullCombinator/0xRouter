//! The request IR (research R3).

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// One generation request, whatever style it arrived in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Request {
    pub model: String,
    /// The top-level system prompt, as text parts in order.
    pub system: Vec<Part>,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
    pub tool_choice: Option<ToolChoice>,
    pub params: Params,
    pub stream: bool,
    /// Top-level client body keys no rule consumed, with their values.
    pub extra: Map<String, Value>,
    /// The path of every key no rule consumed, at any depth (top level, message, part,
    /// tool). A same-style attempt forwards the client's body, so they reach the provider;
    /// a cross-style attempt records them as dropped (research R27). Paths only.
    pub unplaced: Vec<String>,
    /// Content no template recognised. A same-style attempt forwards the client's body; a
    /// cross-style one can't carry it (Constitution IV: content is never dropped).
    pub opaque: Vec<Opaque>,
}

/// A piece of the client body the style couldn't read, with where it was.
#[derive(Debug, Clone, PartialEq)]
pub struct Opaque {
    pub at: String,
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// A system or developer message inside the conversation.
    System,
    User,
    Assistant,
    /// A message that only carries tool results (tool-role layouts).
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "system" => Role::System,
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

/// One content part. `cache_control` is carried as received (not prompt content).
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text {
        text: String,
        cache_control: Option<Value>,
    },
    Image {
        media: Media,
        cache_control: Option<Value>,
    },
    Audio {
        media: Media,
        cache_control: Option<Value>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        cache_control: Option<Value>,
    },
    ToolResult {
        id: String,
        name: Option<String>,
        content: ResultContent,
        is_error: bool,
        cache_control: Option<Value>,
    },
    /// `vendor` is the style that signed it; a signature is only valid on that wire.
    Thinking {
        text: String,
        signature: Option<String>,
        vendor: Option<String>,
    },
}

impl Part {
    pub fn text(text: impl Into<String>) -> Self {
        Part::Text { text: text.into(), cache_control: None }
    }

    pub fn kind(&self) -> nullrouter_registry::schema::PartKind {
        use nullrouter_registry::schema::PartKind as K;
        match self {
            Part::Text { .. } => K::Text,
            Part::Image { .. } => K::Image,
            Part::Audio { .. } => K::Audio,
            Part::ToolCall { .. } => K::ToolCall,
            Part::ToolResult { .. } => K::ToolResult,
            Part::Thinking { .. } => K::Thinking,
        }
    }

    pub fn cache_control(&self) -> Option<&Value> {
        match self {
            Part::Text { cache_control, .. }
            | Part::Image { cache_control, .. }
            | Part::Audio { cache_control, .. }
            | Part::ToolCall { cache_control, .. }
            | Part::ToolResult { cache_control, .. } => cache_control.as_ref(),
            Part::Thinking { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Media {
    pub mime: Option<String>,
    pub source: MediaSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSource {
    /// Base64 payload, without a `data:` prefix.
    Base64(String),
    Url(String),
}

/// A tool result's content, in the form it arrived.
#[derive(Debug, Clone, PartialEq)]
pub enum ResultContent {
    Text(String),
    Parts(Vec<Part>),
    /// A structured result (Gemini `functionResponse.response`).
    Json(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema of the arguments.
    pub parameters: Value,
    pub cache_control: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChoice {
    Auto,
    Required,
    None,
    Named(String),
}

/// Sampling and output parameters. Plain values are keyed by their IR name
/// (`max_tokens`, `temperature`, …, see `IR_PARAMS`); the style's `[text.params]` locates
/// them. `thinking` and `response_format` change shape between styles, so they are typed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Params {
    pub values: BTreeMap<String, Value>,
    pub thinking: Option<Thinking>,
    pub response_format: Option<ResponseFormat>,
}

impl Params {
    pub fn max_tokens(&self) -> Option<u64> {
        self.values.get("max_tokens")?.as_u64()
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Thinking {
    pub enabled: bool,
    pub budget_tokens: Option<u64>,
    /// `low` | `medium` | `high` (OpenAI reasoning effort).
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema { name: Option<String>, schema: Value, strict: Option<bool> },
}

// ── Non-text requests ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EmbeddingsRequest {
    pub model: String,
    pub input: Vec<String>,
    pub params: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImageRequest {
    pub model: String,
    pub prompt: String,
    pub params: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TtsRequest {
    pub model: String,
    pub input: String,
    pub voice: Option<String>,
    pub format: Option<String>,
    pub params: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SttRequest {
    pub model: String,
    pub audio: Media,
    pub language: Option<String>,
    pub params: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VideoRequest {
    pub model: String,
    pub prompt: String,
    pub params: Map<String, Value>,
}
