//! Closed sets that API-style files and schema-2 plugins select by name (spec 003, research
//! R3). Each value names an algorithm or layout the core implements; a file can't add one.

use std::fmt;

use serde::de::{Deserialize, Deserializer, Error as _};
use serde::{Serialize, Serializer};

use super::enums::{CapabilityKind, closed_enum};

closed_enum!(
    /// How a byte stream is cut into frames.
    Framing, "framing" {
        SseNamed = "sse_named",
        SseData = "sse_data",
        SseDataDone = "sse_data_done",
        Ndjson = "ndjson",
        JsonArray = "json_array",
    }
);

closed_enum!(
    /// Whether a style's stream opens and closes content blocks explicitly.
    BlockModel, "block model" {
        Explicit = "explicit",
        Implicit = "implicit",
    }
);

closed_enum!(
    /// Whether tool-call arguments stream as fragments or arrive whole.
    ToolArgumentsMode, "tool arguments mode" {
        Fragments = "fragments",
        Whole = "whole",
    }
);

closed_enum!(
    /// Where a style puts the system prompt.
    SystemLayout, "system layout" {
        TopLevelField = "top_level_field",
        FirstMessage = "first_message",
        InstructionsField = "instructions_field",
        SystemInstruction = "system_instruction",
    }
);

closed_enum!(
    /// Where a style puts an assistant's tool calls.
    ToolCallsLayout, "tool calls layout" {
        ContentPart = "content_part",
        MessageField = "message_field",
        OutputItem = "output_item",
        FunctionCallPart = "function_call_part",
    }
);

closed_enum!(
    /// Where a style puts tool results.
    ToolResultsLayout, "tool results layout" {
        ContentPart = "content_part",
        ToolRoleMessage = "tool_role_message",
        OutputItem = "output_item",
        FunctionResponsePart = "function_response_part",
    }
);

closed_enum!(
    /// How tool-call arguments are carried: a JSON object or a JSON-encoded string.
    ArgumentsForm, "arguments form" {
        JsonObject = "json_object",
        JsonString = "json_string",
    }
);

closed_enum!(
    /// Whether adjacent same-role messages are merged when encoding.
    Alternation, "alternation" {
        MergeAdjacent = "merge_adjacent",
        AsIs = "as_is",
    }
);

closed_enum!(
    /// How a tool result names the call it answers.
    ToolResultMatch, "tool result match" {
        Id = "id",
        Name = "name",
    }
);

closed_enum!(
    /// How message content is written when it is text only.
    ContentForm, "content form" {
        StringWhenTextOnly = "string_when_text_only",
        AlwaysParts = "always_parts",
    }
);

closed_enum!(
    /// How a tool result's content is written.
    ToolResultContent, "tool result content" {
        String = "string",
        Parts = "parts",
        StringOrParts = "string_or_parts",
        Object = "object",
    }
);

closed_enum!(
    /// How an image or audio part is encoded.
    MediaCodec, "media codec" {
        DataUrl = "data_url",
        AnthropicSource = "anthropic_source",
        GeminiInlineData = "gemini_inline_data",
        Url = "url",
    }
);

closed_enum!(
    /// How a thinking request is expressed.
    ThinkingForm, "thinking form" {
        BudgetTokens = "budget_tokens",
        Effort = "effort",
        GeminiThinkingConfig = "gemini_thinking_config",
    }
);

closed_enum!(
    /// How a structured-output request is expressed.
    ResponseFormatForm, "response format form" {
        ChatResponseFormat = "chat_response_format",
        ResponsesTextFormat = "responses_text_format",
        GeminiResponseSchema = "gemini_response_schema",
    }
);

closed_enum!(
    /// How an embedding vector is encoded.
    EmbeddingVector, "embedding vector" {
        Float = "float",
        Base64F32le = "base64_f32le",
    }
);

closed_enum!(
    /// How a request body is encoded.
    BodyEncoding, "body encoding" {
        Json = "json",
        Multipart = "multipart",
        Binary = "binary",
    }
);

closed_enum!(
    /// How an asynchronous job is followed.
    AsyncJob, "async job" {
        PollOnClientRequest = "poll_on_client_request",
    }
);

closed_enum!(
    /// Local token estimators.
    TokenEstimator, "token estimator" {
        Estimate9router = "estimate_9router",
    }
);

closed_enum!(
    /// Named session-id extractors for clients that embed it in a structured field.
    SessionExtractor, "session extractor" {
        ClaudeCodeUserId = "claude_code_user_id",
    }
);

closed_enum!(
    /// Cross-style repairs. They run only when client style and wire differ, and never
    /// change message text.
    Repair, "repair" {
        EnsureToolCallIds = "ensure_tool_call_ids",
        FillMissingToolResults = "fill_missing_tool_results",
        GeminiSchemaSanitize = "gemini_schema_sanitize",
        GeminiFunctionNameSanitize = "gemini_function_name_sanitize",
    }
);

closed_enum!(
    /// Collects audio from a chat stream (openrouter TTS through the chat audio modality).
    AudioCollector, "audio collector" {
        ChatAudioDeltaCollect = "chat_audio_delta_collect",
    }
);

closed_enum!(
    /// What a client route does.
    RouteOp, "route op" {
        Generate = "generate",
        CountTokens = "count_tokens",
        ListModels = "list_models",
        GetModel = "get_model",
        JobSubmit = "job_submit",
        JobGet = "job_get",
        JobContent = "job_content",
    }
);

closed_enum!(
    /// The six model types the pipeline serves.
    ModelType, "model type" {
        Text = "text",
        Embeddings = "embeddings",
        Image = "image",
        Tts = "tts",
        Stt = "stt",
        Video = "video",
    }
);

impl ModelType {
    /// The slice 002 capability that declares models of this type.
    pub fn capability(self) -> CapabilityKind {
        match self {
            Self::Text => CapabilityKind::Llm,
            Self::Embeddings => CapabilityKind::Embedding,
            Self::Image => CapabilityKind::Image,
            Self::Tts => CapabilityKind::Tts,
            Self::Stt => CapabilityKind::Stt,
            Self::Video => CapabilityKind::Video,
        }
    }

    /// The type a slice 002 capability serves, if the pipeline serves it.
    pub fn from_capability(kind: CapabilityKind) -> Option<Self> {
        Some(match kind {
            CapabilityKind::Llm | CapabilityKind::ImageToText => Self::Text,
            CapabilityKind::Embedding => Self::Embeddings,
            CapabilityKind::Image => Self::Image,
            CapabilityKind::Tts => Self::Tts,
            CapabilityKind::Stt => Self::Stt,
            CapabilityKind::Video => Self::Video,
            _ => return None,
        })
    }
}

closed_enum!(
    /// How a style carries a key value in a header.
    KeyScheme, "key scheme" {
        Raw = "raw",
        Bearer = "bearer",
    }
);

closed_enum!(
    /// How a provider continues a cut answer.
    ContinuationMethod, "continuation method" {
        AssistantPrefill = "assistant_prefill",
        PrefixFlag = "prefix_flag",
    }
);

closed_enum!(
    /// Conditions under which continuation is not attempted.
    ContinuationUnless, "continuation condition" {
        ThinkingEnabled = "thinking_enabled",
        ToolCallInProgress = "tool_call_in_progress",
    }
);

closed_enum!(
    /// How a provider session header value is derived from the agent id.
    SessionDerive, "session derive" {
        SesSha256Hex32 = "ses_sha256_hex32",
        SesTimeBase62 = "ses_time_base62",
    }
);

closed_enum!(
    /// How a forwarded client header combines with a plugin's static value.
    ForwardMerge, "forward merge" {
        Replace = "replace",
        AppendCsv = "append_csv",
    }
);

closed_enum!(
    /// Operator choice for a stream that breaks after output reached the client.
    BreakBehaviour, "break behaviour" {
        Restart = "restart",
        ErrorEvent = "error_event",
    }
);

closed_enum!(
    /// Whether a style's reported input tokens include cached tokens.
    InputSemantics, "input semantics" {
        IncludesCache = "includes_cache",
        ExcludesCache = "excludes_cache",
    }
);

closed_enum!(
    /// IR content part kinds; style files key their part templates by these.
    PartKind, "part kind" {
        Text = "text",
        Image = "image",
        Audio = "audio",
        ToolCall = "tool_call",
        ToolResult = "tool_result",
        Thinking = "thinking",
    }
);

closed_enum!(
    /// IR stream events a style maps to its own events (`[[text.stream.events]] on`).
    StreamOn, "stream event" {
        Preamble = "preamble",
        BlockStartText = "block_start:text",
        BlockStartThinking = "block_start:thinking",
        BlockStartToolCall = "block_start:tool_call",
        TextDelta = "text_delta",
        ThinkingDelta = "thinking_delta",
        Signature = "signature",
        ToolArguments = "tool_arguments",
        BlockStop = "block_stop",
        BlockStopText = "block_stop:text",
        BlockStopThinking = "block_stop:thinking",
        BlockStopToolCall = "block_stop:tool_call",
        Usage = "usage",
        Finish = "finish",
        Error = "error",
        Keepalive = "keepalive",
        Done = "done",
    }
);

closed_enum!(
    /// IR finish reasons.
    FinishReason, "finish reason" {
        Stop = "stop",
        Length = "length",
        ToolCalls = "tool_calls",
        ContentFilter = "content_filter",
        Error = "error",
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_primitive_lists_allowed() {
        let err = toml::from_str::<std::collections::BTreeMap<String, Framing>>("f = \"xml\"").unwrap_err().to_string();
        assert!(err.contains("unknown framing \"xml\"; allowed: sse_named, sse_data"), "{err}");
    }

    #[test]
    fn model_type_maps_to_capability() {
        for t in ModelType::ALLOWED.iter().map(|s| ModelType::parse(s).unwrap()) {
            assert_eq!(ModelType::from_capability(t.capability()), Some(t));
        }
        assert_eq!(ModelType::from_capability(CapabilityKind::WebSearch), None);
    }
}
