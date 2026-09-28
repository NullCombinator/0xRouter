//! Views in 9router's shapes: the composed transport (`buildTransport`), the alias maps,
//! and the OAuth URL groups. They exist for parity checks and for the execution slice.

use std::borrow::Cow;
use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::Serialize;
use serde::ser::{SerializeMap, SerializeSeq, Serializer};

use crate::credentials::{ResolvedCredential, SecretString};
use crate::registry::Registry;
use crate::schema::{
    AuthPlacement, CopilotParams, Endpoint, ExecutorParams, ModelType, ProviderEntity, RetryPolicy, StringOrList,
    Transport, TransportAuth, WireFormat,
};

/// The plugin transport as the executor will use it: `format` defaulted, the OAuth
/// `client_id`/`token_url` filled in where the transport lacks them, and the bundled
/// client secret when its bound hosts match (FR-012a).
///
/// Serialises to 9router's camelCase transport shape. `client_secret` is never serialised.
/// For a schema-2 provider the transport is derived from its text endpoints.
#[derive(Debug, Clone)]
pub struct ComposedTransport<'a> {
    pub transport: Cow<'a, Transport>,
    /// The provider's alternative transports, as declared.
    pub transports: Cow<'a, [Transport]>,
    pub format: WireFormat,
    pub client_id: Option<&'a str>,
    pub token_url: Option<&'a str>,
    pub client_secret: Option<&'a SecretString>,
}

/// Anthropic's API authorize endpoint. 9router keeps it apart from the claude plugin's
/// `authorize_url` (the claude.ai login page).
const ANTHROPIC_API_AUTHORIZE: &str = "https://api.anthropic.com/v1/oauth/authorize";
const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_AUTH: &str = "https://accounts.google.com/o/oauth2/auth";

/// The five groups of 9router's `verify-oauth-urls.mjs`. Absent values are omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthUrlsView<'a> {
    pub oauth_endpoints: IndexMap<&'static str, IndexMap<&'static str, &'a str>>,
    pub token_urls: IndexMap<&'static str, &'a str>,
    pub auth_urls: IndexMap<&'static str, &'a str>,
    pub refresh_urls: IndexMap<&'static str, &'a str>,
    pub client_ids: IndexMap<&'static str, &'a str>,
}

impl Registry {
    /// `None` for a catalog-only provider (no transport) or an unknown token.
    pub fn composed_transport(&self, provider: &str) -> Option<ComposedTransport<'_>> {
        let p = self.provider(provider).ok()?;
        let (t, transports) = match &p.transport {
            Some(t) => (Cow::Borrowed(t), Cow::Borrowed(p.transports.as_slice())),
            None => {
                let mut derived = p.endpoints.get(&ModelType::Text)?.0.iter().map(|e| from_endpoint(p, e));
                (Cow::Owned(derived.next()?), Cow::Owned(derived.collect()))
            }
        };
        let oauth = p.oauth.as_ref();
        let declared = p.transport.as_ref();
        Some(ComposedTransport {
            format: t.format.unwrap_or(WireFormat::Openai),
            client_id: declared.and_then(|t| t.client_id.as_deref()).or_else(|| oauth?.client_id.as_deref()),
            token_url: declared.and_then(|t| t.token_url.as_deref()).or_else(|| oauth?.token_url.as_deref()),
            client_secret: match self.credentials.get(&p.id) {
                Some(ResolvedCredential::Available(s)) => Some(*s),
                _ => None,
            },
            transport: t,
            transports,
        })
    }

    /// The provider id a lookup token names.
    pub fn alias_view(&self, token: &str) -> Option<&str> {
        self.provider(token).ok().map(|p| p.id.as_str())
    }

    /// 9router `PROVIDER_ID_TO_ALIAS`: every provider with a transport → `alias`, else id.
    pub fn id_to_alias(&self) -> BTreeMap<&str, &str> {
        self.providers()
            .filter(|p| p.has_text_transport())
            .map(|p| (p.id.as_str(), p.alias.as_deref().unwrap_or(&p.id)))
            .collect()
    }

    pub fn oauth_urls_view(&self) -> OAuthUrlsView<'_> {
        let oauth = |id: &str| self.provider(id).ok().and_then(|p| p.oauth.as_ref());
        let composed = |id: &str| self.composed_transport(id);
        // OAuth providers are schema 1, so their declared transport is the composed one.
        let declared = |id: &str| self.provider(id).ok()?.transport.as_ref();

        let codex = oauth("codex");
        let claude = oauth("claude");
        let iflow = oauth("iflow");
        let github = oauth("github");
        let oauth_endpoints = [
            ("google", group(&[("token", Some(GOOGLE_TOKEN)), ("auth", Some(GOOGLE_AUTH))])),
            (
                "openai",
                group(&[
                    ("token", codex.and_then(|o| o.token_url.as_deref())),
                    ("auth", codex.and_then(|o| o.authorize_url.as_deref())),
                ]),
            ),
            (
                "anthropic",
                group(&[
                    ("token", claude.and_then(|o| o.token_url.as_deref())),
                    ("auth", Some(ANTHROPIC_API_AUTHORIZE)),
                ]),
            ),
            (
                "iflow",
                group(&[
                    ("token", iflow.and_then(|o| o.token_url.as_deref())),
                    ("auth", iflow.and_then(|o| o.authorize_url.as_deref())),
                ]),
            ),
            (
                "github",
                group(&[
                    ("token", github.and_then(|o| o.token_url.as_deref())),
                    ("auth", github.and_then(|o| o.authorize_url.as_deref())),
                    ("deviceCode", github.and_then(|o| o.device_code_url.as_deref())),
                ]),
            ),
        ]
        .into_iter()
        .collect();

        OAuthUrlsView {
            oauth_endpoints,
            token_urls: pick(&["claude", "codex", "iflow", "kiro", "xai", "grok-cli", "cline", "kimi"], |id| {
                composed(id)?.token_url
            }),
            auth_urls: pick(&["iflow", "kiro"], |id| declared(id)?.auth_url.as_deref()),
            // 9router lists grok-cli's token URL as its refresh URL.
            refresh_urls: pick(&["cline", "kimi", "xai", "grok-cli"], |id| {
                if id == "grok-cli" { composed(id)?.token_url } else { declared(id)?.refresh_url.as_deref() }
            }),
            client_ids: pick(&["claude", "codex", "iflow", "kimi", "grok-cli"], |id| composed(id)?.client_id),
        }
    }
}

/// A schema-1 transport equivalent of one schema-2 text endpoint (the conversion table in
/// contracts/provider-schema-v2.md, read backwards). `None` format means an unmapped wire.
fn from_endpoint(p: &ProviderEntity, e: &Endpoint) -> Transport {
    let format = e.wire.as_deref().and_then(WireFormat::from_wire);
    let retry = e.retry.iter().map(|(code, r)| {
        (code.clone(), RetryPolicy::Policy { attempts: r.retries, delay_ms: Some(r.delay_ms) })
    });
    let auth = match &e.auth {
        Some(a) => Some(TransportAuth { header: Some(a.header.clone()), scheme: Some(a.scheme), ..TransportAuth::default() }),
        None => p.auth.as_ref().filter(|a| a.header.is_some() || a.scheme.is_some()).map(|a| TransportAuth {
            header: a.header.clone(),
            scheme: a.scheme,
            ..TransportAuth::default()
        }),
    };
    Transport {
        base_url: Some(e.url.clone()),
        format,
        headers: (!e.headers.is_empty()).then(|| e.headers.clone()),
        force_stream: e.force_stream.then_some(true),
        timeout_ms: e.timeout_ms,
        stall_timeout_ms: e.stall_timeout_ms,
        retry: (!e.retry.is_empty()).then(|| retry.collect()),
        auth,
        ..Transport::default()
    }
}

fn group<'a>(pairs: &[(&'static str, Option<&'a str>)]) -> IndexMap<&'static str, &'a str> {
    pairs.iter().filter_map(|&(k, v)| Some((k, v?))).collect()
}

fn pick<'a>(ids: &[&'static str], f: impl Fn(&str) -> Option<&'a str>) -> IndexMap<&'static str, &'a str> {
    ids.iter().filter_map(|&id| Some((id, f(id)?))).collect()
}

// ── Serialisation: the generator's key mapping, reversed ─────────────────────────

const QUIRKS_JS: &[(&str, &str)] = &[
    ("preserve_cache_control", "preserveCacheControl"),
    ("drop_client_metadata", "dropClientMetadata"),
    ("cline_envelope", "clineEnvelope"),
    ("drop_output_config", "dropOutputConfig"),
    ("require_claude_tool_type", "requireClaudeToolType"),
    ("cloak_tools_on_oauth", "cloakToolsOnOAuth"),
];

const HOOKS_JS: &[(&str, &str)] =
    &[("cline_headers", "clineHeaders"), ("kimi_headers", "kimiHeaders"), ("kilocode_org", "kilocodeOrg")];

fn js_name(table: &[(&str, &'static str)], name: &'static str) -> &'static str {
    table.iter().find(|(k, _)| *k == name).map_or(name, |(_, v)| v)
}

impl Serialize for ComposedTransport<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        entries(&mut map, &self.transport, Some(self))?;
        map.end()
    }
}

/// A transport serialised as declared, without composition (`transports[]` entries).
struct Raw<'a>(&'a Transport);

impl Serialize for Raw<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        entries(&mut map, self.0, None)?;
        map.end()
    }
}

fn opt<M: SerializeMap, T: Serialize + ?Sized>(map: &mut M, key: &str, v: Option<&T>) -> Result<(), M::Error> {
    match v {
        Some(v) => map.serialize_entry(key, v),
        None => Ok(()),
    }
}

fn entries<M: SerializeMap>(map: &mut M, t: &Transport, c: Option<&ComposedTransport<'_>>) -> Result<(), M::Error> {
    opt(map, "baseUrl", t.base_url.as_deref())?;
    opt(map, "baseUrls", t.base_urls.as_ref())?;
    opt(map, "format", c.map(|c| &c.format).or(t.format.as_ref()))?;
    opt(map, "headers", t.headers.as_ref())?;
    opt(map, "forceStream", t.force_stream.as_ref())?;
    opt(map, "urlSuffix", t.url_suffix.as_deref())?;
    opt(map, "timeoutMs", t.timeout_ms.as_ref())?;
    opt(map, "stallTimeoutMs", t.stall_timeout_ms.as_ref())?;
    opt(map, "thinkingFormat", t.thinking_format.as_deref())?;
    opt(map, "validateUrl", t.validate_url.as_deref())?;
    opt(map, "modelsUrl", t.models_url.as_deref())?;
    opt(map, "responsesUrl", t.responses_url.as_deref())?;
    opt(map, "messagesUrl", t.messages_url.as_deref())?;
    opt(map, "chatPath", t.chat_path.as_deref())?;
    opt(map, "userUrl", t.user_url.as_deref())?;
    opt(map, "billingUrl", t.billing_url.as_deref())?;
    opt(map, "refreshUrl", t.refresh_url.as_deref())?;
    opt(map, "tokenUrl", c.map_or(t.token_url.as_deref(), |c| c.token_url))?;
    opt(map, "authUrl", t.auth_url.as_deref())?;
    opt(map, "clientId", c.map_or(t.client_id.as_deref(), |c| c.client_id))?;
    opt(map, "retry", t.retry.as_ref().map(Retry).as_ref())?;
    opt(map, "reasoningInject", t.reasoning_inject.as_ref())?;
    opt(map, "usage", t.usage.as_ref().map(Usage).as_ref())?;
    opt(map, "regions", t.regions.as_ref())?;
    opt(map, "defaultRegion", t.default_region.as_deref())?;
    opt(map, "auth", t.auth.as_ref().map(Auth).as_ref())?;
    if !t.quirks.is_empty() || t.claude_supported_tool_types.is_some() || t.force_auto_tool_choice_models.is_some() {
        map.serialize_entry("quirks", &Quirks(t))?;
    }
    if let Some(e) = &t.executor_params {
        executor_params(map, e)?;
    }
    match c {
        Some(c) if !c.transports.is_empty() => map.serialize_entry("transports", &RawList(&c.transports)),
        _ => Ok(()),
    }
}

fn executor_params<M: SerializeMap>(map: &mut M, e: &ExecutorParams) -> Result<(), M::Error> {
    opt(map, "cliVersion", e.cli_version.as_deref())?;
    opt(map, "clientVersion", e.client_version.as_deref())?;
    opt(map, "apiClient", e.api_client.as_deref())?;
    opt(map, "clientIdentifier", e.client_identifier.as_deref())?;
    opt(map, "tokenAuth", e.token_auth.as_deref())?;
    opt(map, "noAuth", e.no_auth.as_ref())?;
    opt(map, "authType", e.auth_type.as_deref())?;
    opt(map, "copilot", e.copilot.as_ref().map(Copilot).as_ref())
}

struct Retry<'a>(&'a IndexMap<String, RetryPolicy>);

impl Serialize for Retry<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (code, p) in self.0 {
            match p {
                RetryPolicy::Attempts(n) => map.serialize_entry(code, n)?,
                RetryPolicy::Policy { attempts, delay_ms } => {
                    map.serialize_entry(code, &RetryObj { attempts: *attempts, delay_ms: *delay_ms })?
                }
            }
        }
        map.end()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RetryObj {
    attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    delay_ms: Option<u64>,
}

struct Usage<'a>(&'a IndexMap<String, StringOrList>);

impl Serialize for Usage<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            match v {
                StringOrList::One(one) => map.serialize_entry(k, one)?,
                StringOrList::Many(many) => map.serialize_entry(k, many)?,
            }
        }
        map.end()
    }
}

struct Auth<'a>(&'a TransportAuth);

impl Serialize for Auth<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let a = self.0;
        let mut map = s.serialize_map(None)?;
        opt(&mut map, "combined", a.combined.as_ref())?;
        opt(&mut map, "header", a.header.as_deref())?;
        opt(&mut map, "scheme", a.scheme.as_ref())?;
        if !a.hooks.is_empty() {
            let hooks: Vec<&str> = a.hooks.iter().map(|h| js_name(HOOKS_JS, h.as_str())).collect();
            map.serialize_entry("hooks", &hooks)?;
        }
        opt(&mut map, "apiKey", a.api_key.as_ref().map(Placement).as_ref())?;
        opt(&mut map, "oauth", a.oauth.as_ref().map(Placement).as_ref())?;
        opt(&mut map, "anthropicVersion", a.anthropic_version.as_ref())?;
        map.end()
    }
}

struct Placement<'a>(&'a AuthPlacement);

impl Serialize for Placement<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(2))?;
        map.serialize_entry("header", &self.0.header)?;
        map.serialize_entry("scheme", &self.0.scheme)?;
        map.end()
    }
}

struct Quirks<'a>(&'a Transport);

impl Serialize for Quirks<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let t = self.0;
        let mut map = s.serialize_map(None)?;
        for q in &t.quirks {
            map.serialize_entry(js_name(QUIRKS_JS, q.as_str()), &true)?;
        }
        opt(&mut map, "claudeSupportedToolTypes", t.claude_supported_tool_types.as_ref())?;
        opt(&mut map, "forceAutoToolChoiceModels", t.force_auto_tool_choice_models.as_ref())?;
        map.end()
    }
}

struct Copilot<'a>(&'a CopilotParams);

impl Serialize for Copilot<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let c = self.0;
        let mut map = s.serialize_map(None)?;
        opt(&mut map, "vscodeVersion", c.vscode_version.as_deref())?;
        opt(&mut map, "chatVersion", c.chat_version.as_deref())?;
        opt(&mut map, "userAgent", c.user_agent.as_deref())?;
        opt(&mut map, "apiVersion", c.api_version.as_deref())?;
        map.end()
    }
}

/// `transports[]` of a provider, serialised as declared.
struct RawList<'a>(&'a [Transport]);

impl Serialize for RawList<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for t in self.0 {
            seq.serialize_element(&Raw(t))?;
        }
        seq.end()
    }
}
