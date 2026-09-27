//! The plugin file and the provider entity it declares (data-model § ProviderEntity).

use std::collections::BTreeMap;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::Deserialize;

use super::capability::CapabilitySection;
use super::enums::{AuthHook, AuthKind, AuthScheme, CapabilityKind, Category};
use super::model::{Model, de_models};
use super::oauth::OAuthDecl;
use super::transport::Transport;

/// One plugin file, as parsed. Only `id` and `category` are required (FR-006).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginFile {
    pub schema: Option<i64>,
    pub id: String,
    pub category: Category,
    pub alias: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub ui_alias: Option<String>,
    #[serde(default)]
    pub passthrough_models: bool,
    #[serde(default)]
    pub version_separator_tolerance: bool,
    pub auth: Option<AuthDecl>,
    pub transport: Option<Transport>,
    #[serde(default)]
    pub transports: Vec<Transport>,
    pub oauth: Option<OAuthDecl>,
    pub models_fetcher: Option<ModelsFetcher>,
    /// `None` = catalog unknown; `Some([])` = offers no models (FR-005).
    #[serde(default, deserialize_with = "de_models")]
    pub models: Option<Vec<Model>>,
    #[serde(default)]
    pub capabilities: BTreeMap<CapabilityKind, CapabilitySection>,
    pub display: Option<Display>,
}

/// A validated provider, as the registry holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderEntity {
    pub id: String,
    pub category: Category,
    pub alias: Option<String>,
    pub aliases: Vec<String>,
    pub ui_alias: Option<String>,
    pub passthrough_models: bool,
    pub version_separator_tolerance: bool,
    pub auth: Option<AuthDecl>,
    pub transport: Option<Transport>,
    pub transports: Vec<Transport>,
    pub oauth: Option<OAuthDecl>,
    pub models_fetcher: Option<ModelsFetcher>,
    pub models: Option<Vec<Model>>,
    pub capabilities: BTreeMap<CapabilityKind, CapabilitySection>,
    pub display: Option<Display>,
    pub source: PluginSource,
}

impl ProviderEntity {
    pub(crate) fn from_file(f: PluginFile, source: PluginSource) -> Self {
        Self {
            id: f.id,
            category: f.category,
            alias: f.alias,
            aliases: f.aliases,
            ui_alias: f.ui_alias,
            passthrough_models: f.passthrough_models,
            version_separator_tolerance: f.version_separator_tolerance,
            auth: f.auth,
            transport: f.transport,
            transports: f.transports,
            oauth: f.oauth,
            models_fetcher: f.models_fetcher,
            models: f.models,
            capabilities: f.capabilities,
            display: f.display,
            source,
        }
    }

    /// Lookup tokens this provider owns: its id, `alias`, and `aliases` (never `ui_alias`).
    pub fn tokens(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.id.as_str())
            .chain(self.alias.as_deref())
            .chain(self.aliases.iter().map(String::as_str))
    }

    /// `transport` followed by every `transports[]` entry.
    pub fn all_transports(&self) -> Vec<&Transport> {
        self.transport.iter().chain(&self.transports).collect()
    }

    pub fn is_bundled(&self) -> bool {
        matches!(self.source, PluginSource::Bundled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    Bundled,
    User(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthDecl {
    pub kind: Option<AuthKind>,
    #[serde(default)]
    pub modes: Vec<AuthKind>,
    #[serde(default)]
    pub no_auth: bool,
    #[serde(default)]
    pub has_oauth: bool,
    pub header: Option<String>,
    pub scheme: Option<AuthScheme>,
    #[serde(default)]
    pub hooks: Vec<AuthHook>,
    /// Reuse another provider's user credential. Checked against the active set at load.
    pub credential_fallback: Option<String>,
    /// UI hint shown next to the API-key field.
    pub api_key_hint: Option<String>,
}

/// Where the dashboard fetches a live model list. `kind` names a core fetcher.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelsFetcher {
    pub kind: String,
    pub url: String,
}

/// UI metadata carried for the dashboard; not interpreted by this slice.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display {
    pub name: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub text_icon: Option<String>,
    pub website: Option<String>,
    pub notice: Option<Notice>,
    pub kind_notice: Option<IndexMap<String, String>>,
    pub deprecated: Option<bool>,
    pub deprecation_notice: Option<String>,
    pub priority: Option<i64>,
    pub hidden: Option<bool>,
    pub has_free: Option<bool>,
    pub auth_hint: Option<String>,
    pub has_provider_specific_data: Option<bool>,
    pub regions: Option<Vec<Region>>,
    pub default_region: Option<String>,
    pub media_priority: Option<i64>,
    pub features: Option<Features>,
    pub thinking: Option<ThinkingDisplay>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub api_key_url: Option<String>,
    pub signup_url: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Features {
    pub usage: Option<bool>,
    pub usage_apikey: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThinkingDisplay {
    #[serde(default)]
    pub options: Vec<String>,
    pub default_mode: Option<String>,
}
