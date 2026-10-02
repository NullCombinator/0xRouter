//! Per-capability sections (data-model § CapabilitySection).

use indexmap::IndexMap;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySection {
    /// `None` = served by the provider's main `transport`.
    pub endpoint: Option<SectionEndpoint>,
    /// Capability-owned model lists (TTS models, embedding models with dimensions).
    pub models: Option<Vec<SectionModel>>,
    /// TTS voice tables.
    pub voices: Option<Vec<SectionModel>>,
    pub limits: Option<SectionLimits>,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionEndpoint {
    pub base_url: Option<String>,
    /// `apikey` or `none`.
    pub auth_type: Option<String>,
    /// How the core authenticates: `bearer`, `basic`, `aws-sigv4`, a header name, ….
    pub auth_header: Option<String>,
    /// Names a core media handler; must be in [`KNOWN_SECTION_FORMATS`](super::KNOWN_SECTION_FORMATS).
    pub format: Option<String>,
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    pub method: Option<String>,
    pub timeout_ms: Option<u64>,
    pub default_model: Option<String>,
    pub poll_url: Option<String>,
    pub validate_url: Option<String>,
    pub body_fields: Option<Vec<String>>,
    /// Model id → provider path, or `{ path, task }`.
    pub model_map: Option<IndexMap<String, ModelRoute>>,
    pub search_types: Option<Vec<String>>,
    pub formats: Option<Vec<String>>,
    /// Web search is answered by the chat endpoint (9router `searchViaChat`).
    #[serde(default)]
    pub via_chat: bool,
    pub pricing_url: Option<String>,
    pub free_tier_note: Option<String>,
}

impl SectionEndpoint {
    pub(crate) fn url_fields(&self) -> impl Iterator<Item = (String, &str)> {
        [
            ("base_url", &self.base_url),
            ("poll_url", &self.poll_url),
            ("validate_url", &self.validate_url),
            ("pricing_url", &self.pricing_url),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.as_deref().map(|v| (k.to_owned(), v)))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ModelRoute {
    Path(String),
    Task { path: String, task: String },
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionLimits {
    pub cost_per_query: Option<f64>,
    pub free_monthly_quota: Option<u64>,
    pub max_max_results: Option<u64>,
    pub default_max_results: Option<u64>,
    pub cache_ttl_ms: Option<u64>,
    pub credits_per_result: Option<f64>,
    pub max_characters: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionModel {
    pub id: String,
    pub name: Option<String>,
    pub dimensions: Option<u64>,
}
