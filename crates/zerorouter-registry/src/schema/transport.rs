//! Upstream endpoint declaration (data-model § Transport).
//!
//! URLs stay as the declared strings: the composed view must reproduce them byte for
//! byte, and `url::Url` normalises. The gate parses and checks every one of them.

use indexmap::IndexMap;
use serde::Deserialize;

use super::enums::{AuthHook, AuthScheme, Quirk, WireFormat};

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transport {
    pub base_url: Option<String>,
    pub base_urls: Option<Vec<String>>,
    pub format: Option<WireFormat>,
    /// `Some({})` is kept distinct from absent: 9router emits `headers: {}` for some providers.
    pub headers: Option<IndexMap<String, String>>,
    pub force_stream: Option<bool>,
    pub url_suffix: Option<String>,
    pub timeout_ms: Option<u64>,
    pub stall_timeout_ms: Option<u64>,
    pub thinking_format: Option<String>,
    pub validate_url: Option<String>,
    pub models_url: Option<String>,
    pub responses_url: Option<String>,
    pub messages_url: Option<String>,
    pub chat_path: Option<String>,
    pub user_url: Option<String>,
    pub billing_url: Option<String>,
    pub refresh_url: Option<String>,
    pub token_url: Option<String>,
    pub auth_url: Option<String>,
    pub client_id: Option<String>,
    /// Keys are HTTP status codes. Carried for the execution slice.
    pub retry: Option<IndexMap<String, RetryPolicy>>,
    pub reasoning_inject: Option<IndexMap<String, String>>,
    /// Usage-reporting endpoints and paths.
    pub usage: Option<IndexMap<String, StringOrList>>,
    pub regions: Option<IndexMap<String, String>>,
    pub default_region: Option<String>,
    pub auth: Option<TransportAuth>,
    #[serde(default)]
    pub quirks: Vec<Quirk>,
    pub claude_supported_tool_types: Option<Vec<String>>,
    pub force_auto_tool_choice_models: Option<Vec<String>>,
    pub executor_params: Option<ExecutorParams>,
}

impl Transport {
    /// Every absolute-URL field, with its key, for the gate's URL checks.
    pub(crate) fn url_fields(&self) -> impl Iterator<Item = (String, &str)> {
        let single = [
            ("base_url", &self.base_url),
            ("validate_url", &self.validate_url),
            ("models_url", &self.models_url),
            ("responses_url", &self.responses_url),
            ("messages_url", &self.messages_url),
            ("user_url", &self.user_url),
            ("billing_url", &self.billing_url),
            ("refresh_url", &self.refresh_url),
            ("token_url", &self.token_url),
            ("auth_url", &self.auth_url),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.as_deref().map(|v| (k.to_owned(), v)));
        let lists = self
            .base_urls
            .iter()
            .flatten()
            .enumerate()
            .map(|(i, u)| (format!("base_urls[{i}]"), u.as_str()));
        let regions = self
            .regions
            .iter()
            .flatten()
            .map(|(k, u)| (format!("regions.{k}"), u.as_str()));
        single.chain(lists).chain(regions)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum RetryPolicy {
    Attempts(u32),
    Policy {
        attempts: u32,
        delay_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum StringOrList {
    One(String),
    Many(Vec<String>),
}

/// Where the core puts the credential for this endpoint.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportAuth {
    /// One header serves both API-key and OAuth credentials.
    pub combined: Option<bool>,
    pub header: Option<String>,
    pub scheme: Option<AuthScheme>,
    #[serde(default)]
    pub hooks: Vec<AuthHook>,
    pub api_key: Option<AuthPlacement>,
    pub oauth: Option<AuthPlacement>,
    /// Send the `Anthropic-Version` header on this transport.
    pub anthropic_version: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthPlacement {
    pub header: String,
    pub scheme: AuthScheme,
}

/// Long-tail, executor-specific values. A closed key set, not a free map.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorParams {
    pub cli_version: Option<String>,
    pub client_version: Option<String>,
    pub api_client: Option<String>,
    pub client_identifier: Option<String>,
    /// An auth-mode name such as `"xai-grok-cli"`, not a credential.
    pub token_auth: Option<String>,
    pub no_auth: Option<bool>,
    pub auth_type: Option<String>,
    pub copilot: Option<CopilotParams>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopilotParams {
    pub vscode_version: Option<String>,
    pub chat_version: Option<String>,
    pub user_agent: Option<String>,
    pub api_version: Option<String>,
}
