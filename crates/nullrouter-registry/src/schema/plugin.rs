//! The plugin file and the provider entity it declares (data-model § ProviderEntity).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::Deserialize;
use url::Url;

use super::capability::CapabilitySection;
use super::endpoint::Endpoints;
use super::enums::{AuthHook, AuthKind, AuthScheme, CapabilityKind, Category, WireFormat};
use super::forwarding::Forwarding;
use super::identity::IdentityDecl;
use super::model::{Model, de_models};
use super::models_live::ModelsLiveDecl;
use super::oauth::OAuthDecl;
use super::primitives::ModelType;
use super::quota::QuotaDecl;
use super::routing::{EffectiveRouting, RoutingDecl};
use super::session::ProviderSession;
use super::signin::SignInDecl;
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
    /// Schema 2: upstream endpoints per model type.
    #[serde(default)]
    pub endpoints: BTreeMap<ModelType, Endpoints>,
    pub forwarding: Option<Forwarding>,
    pub session: Option<ProviderSession>,
    /// Schema 2: account sign-in (slice 005). Bundled plugins only.
    pub signin: Option<SignInDecl>,
    /// Schema 2: headers on a sign-in account's requests. Bundled plugins only.
    pub identity: Option<IdentityDecl>,
    /// Schema 2: how an account's quota is read. Bundled plugins only.
    pub quota: Option<QuotaDecl>,
    /// Schema 2: the live model list. Bundled plugins only.
    pub models_live: Option<ModelsLiveDecl>,
    /// Schema 2: prompt-cache behaviour, quota meters and prices (slice 006).
    pub routing: Option<RoutingDecl>,
    /// Inert names used only by the fit check.
    #[serde(default)]
    pub requires: Vec<String>,
}

impl PluginFile {
    pub(crate) fn auth_headers(&self) -> impl Iterator<Item = &str> {
        auth_headers(self.auth.as_ref(), &self.endpoints, self.signin.as_ref())
    }

    /// `schema`, defaulting to 1.
    pub fn schema_version(&self) -> i64 {
        self.schema.unwrap_or(1)
    }

    /// Schema-1 execution keys present in this file, by path. Schema 2 declares these
    /// under `endpoints`.
    pub fn schema1_execution_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        if self.transport.is_some() {
            keys.push("transport".to_owned());
        }
        if !self.transports.is_empty() {
            keys.push("transports".to_owned());
        }
        for (kind, sec) in &self.capabilities {
            if sec.endpoint.is_some() {
                keys.push(format!("capabilities.{kind}.endpoint"));
            }
        }
        if self.auth.as_ref().is_some_and(|a| !a.hooks.is_empty()) {
            keys.push("auth.hooks".to_owned());
        }
        keys
    }
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
    pub endpoints: BTreeMap<ModelType, Endpoints>,
    pub forwarding: Option<Forwarding>,
    pub session: Option<ProviderSession>,
    pub signin: Option<SignInDecl>,
    pub identity: Option<IdentityDecl>,
    pub quota: Option<QuotaDecl>,
    pub models_live: Option<ModelsLiveDecl>,
    pub routing: Option<RoutingDecl>,
    pub requires: Vec<String>,
    pub source: PluginSource,
}

fn auth_headers<'a>(
    auth: Option<&'a AuthDecl>,
    endpoints: &'a BTreeMap<ModelType, Endpoints>,
    signin: Option<&'a SignInDecl>,
) -> impl Iterator<Item = &'a str> {
    let per_endpoint = endpoints.values().flat_map(|e| &e.0).filter_map(|e| Some(e.auth.as_ref()?.header.as_str()));
    let signed_in = signin.map(|s| s.auth.header.as_str());
    auth.and_then(|a| a.header.as_deref()).into_iter().chain(per_endpoint).chain(signed_in)
}

fn host_of(url: &str) -> Option<String> {
    Url::parse(url).ok()?.host_str().map(str::to_owned)
}

/// Every host the provider's endpoints send requests to: endpoint and token-count URLs, and
/// schema-1 transport base URLs.
pub(crate) fn endpoint_hosts(
    endpoints: &BTreeMap<ModelType, Endpoints>,
    transports: &[&Transport],
) -> BTreeSet<String> {
    let urls = endpoints
        .values()
        .flat_map(|e| e.0.iter())
        .flat_map(|e| std::iter::once(e.url.as_str()).chain(e.token_count.iter().map(|t| t.url.as_str())));
    let bases =
        transports.iter().flat_map(|t| t.base_url.iter().chain(t.base_urls.iter().flatten())).map(String::as_str);
    urls.chain(bases).filter_map(host_of).collect()
}

/// The URLs of `[signin]` (flow and profile), `[quota]` (primary and fallback) and
/// `[models_live]`, by field path.
pub(crate) fn account_urls<'a>(
    signin: Option<&'a SignInDecl>,
    quota: Option<&'a QuotaDecl>,
    models_live: Option<&'a ModelsLiveDecl>,
) -> Vec<(String, &'a str)> {
    let mut out = Vec::new();
    if let Some(s) = signin {
        out.extend(s.flow_urls().map(|(k, u)| (format!("signin.{k}"), u)));
        out.extend(s.profile.iter().map(|p| ("signin.profile.url".to_owned(), p.url.as_str())));
    }
    if let Some(q) = quota {
        out.push(("quota.request.url".to_owned(), q.primary.request.url.as_str()));
        out.extend(q.fallback.iter().map(|f| ("quota.fallback.request.url".to_owned(), f.request.url.as_str())));
    }
    out.extend(models_live.map(|m| ("models_live.url".to_owned(), m.url.as_str())));
    out
}

impl ProviderEntity {
    pub(crate) fn from_file(f: PluginFile, source: PluginSource) -> Self {
        // Schema 2 declares model types through `endpoints`; slice 002's capability view
        // is derived from them.
        let mut capabilities = f.capabilities;
        for (ty, eps) in &f.endpoints {
            capabilities.entry(ty.capability()).or_default();
            if *ty == ModelType::Text && eps.0.iter().any(|e| e.vision) {
                capabilities.entry(CapabilityKind::ImageToText).or_default();
            }
        }
        // Schema 2's per-model `wires` stand in for 9router's `supported_formats`.
        let mut models = f.models;
        for m in models.iter_mut().flatten() {
            if let Some(w) = &m.wires {
                m.supported_formats.get_or_insert_with(|| w.iter().filter_map(|w| WireFormat::from_wire(w)).collect());
            }
        }
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
            models,
            capabilities,
            display: f.display,
            endpoints: f.endpoints,
            forwarding: f.forwarding,
            session: f.session,
            signin: f.signin,
            identity: f.identity,
            quota: f.quota,
            models_live: f.models_live,
            routing: f.routing,
            requires: f.requires,
            source,
        }
    }

    /// The `[routing]` declaration with the contract's defaults filled in (research R16).
    pub fn routing(&self) -> EffectiveRouting<'_> {
        EffectiveRouting::of(self.routing.as_ref())
    }

    /// Lookup tokens this provider owns: its id, `alias`, and `aliases` (never `ui_alias`).
    pub fn tokens(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.id.as_str()).chain(self.alias.as_deref()).chain(self.aliases.iter().map(String::as_str))
    }

    /// A schema 1 `transport` or a schema 2 text endpoint: what 9router calls a provider
    /// with a transport.
    pub fn has_text_transport(&self) -> bool {
        self.transport.is_some() || self.endpoints.contains_key(&ModelType::Text)
    }

    /// Every header name this provider's secret or access token goes into: `[auth]`'s,
    /// each endpoint's, and `[signin] auth`'s.
    pub fn auth_headers(&self) -> impl Iterator<Item = &str> {
        auth_headers(self.auth.as_ref(), &self.endpoints, self.signin.as_ref())
    }

    /// Every host this provider's endpoints send requests to (endpoint and token-count
    /// URLs, schema-1 transport base URLs).
    pub fn endpoint_hosts(&self) -> BTreeSet<String> {
        endpoint_hosts(&self.endpoints, &self.all_transports())
    }

    /// The hosts a sign-in account's tokens are bound to (research R5, FR-031): endpoint
    /// hosts, `[signin]` hosts (flow and profile), `[quota]` hosts (primary and fallback),
    /// and the `[models_live]` host.
    pub fn token_hosts(&self) -> BTreeSet<String> {
        let mut hosts = self.endpoint_hosts();
        let urls = account_urls(self.signin.as_ref(), self.quota.as_ref(), self.models_live.as_ref());
        hosts.extend(urls.into_iter().filter_map(|(_, u)| host_of(u)));
        hosts
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
