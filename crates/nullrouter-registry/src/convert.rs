//! Schema 1 → 2 conversion for community plugins (contracts/provider-schema-v2.md § Schema
//! 1 → 2 conversion). It runs on a plugin the fit check passed, and adds the `endpoints` the
//! engine executes. The schema 1 fields stay, so slice 002's views are unchanged.

use std::collections::BTreeMap;

use indexmap::IndexMap;

use crate::schema::{
    AuthScheme, CapabilityKind, Endpoint, EndpointAuth, Endpoints, ErrorRules, ForceMap, ModelType, ProviderEntity,
    RetryOverride, RetryPolicy, SectionEndpoint, Transport, WireFormat,
};

/// 9router's default `anthropic-version` for Claude-format transports.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Adds `endpoints` to a schema 1 entity that fits. A schema 2 entity is left as is.
pub(crate) fn to_schema2(p: &mut ProviderEntity) {
    if !p.endpoints.is_empty() {
        return;
    }
    let vision = p.capabilities.contains_key(&CapabilityKind::ImageToText);
    let mut text = Vec::new();
    for t in p.transport.iter().chain(&p.transports) {
        for e in text_endpoints(t, vision) {
            if !text.iter().any(|x: &Endpoint| x.url == e.url && x.wire == e.wire) {
                text.push(e);
            }
        }
    }
    if !text.is_empty() {
        p.endpoints.insert(ModelType::Text, Endpoints(text));
    }
    for (kind, sec) in &p.capabilities {
        let (Some(t), Some(e)) = (ModelType::from_capability(*kind), &sec.endpoint) else { continue };
        if t != ModelType::Text
            && let Some(ep) = media_endpoint(e)
        {
            p.endpoints.insert(t, Endpoints(vec![ep]));
        }
    }
    for m in p.models.iter_mut().flatten() {
        let formats = m.target_format.map(|f| vec![f]).or_else(|| m.supported_formats.clone());
        if m.wires.is_none() {
            m.wires = formats.map(|fs| fs.iter().filter_map(|f| f.wire()).map(str::to_owned).collect());
        }
    }
}

fn endpoint(url: String, wire: Option<&str>) -> Endpoint {
    Endpoint {
        url,
        method: "POST".into(),
        wire: wire.map(str::to_owned),
        body: None,
        response: None,
        headers: IndexMap::new(),
        auth: None,
        encoding: None,
        timeout_ms: None,
        stall_timeout_ms: None,
        force_stream: false,
        vision: false,
        retry: BTreeMap::new(),
        models: Vec::new(),
        voices: Vec::new(),
        errors: ErrorRules::default(),
        token_count: None,
        continuation: None,
        force: ForceMap::default(),
    }
}

/// One endpoint per URL of a transport (`base_url`, else each of `base_urls`).
fn text_endpoints(t: &Transport, vision: bool) -> Vec<Endpoint> {
    let format = t.format.unwrap_or(WireFormat::Openai);
    let Some(wire) = format.wire() else { return Vec::new() };
    let urls: Vec<&String> = t.base_url.iter().chain(t.base_urls.iter().flatten()).collect();
    urls.into_iter()
        .map(|base| {
            let mut e = endpoint(String::new(), Some(wire));
            // 9router switches Gemini's URL per request; the core streams and aggregates.
            e.url = match format {
                WireFormat::Gemini => format!("{base}/{{model}}:streamGenerateContent?alt=sse"),
                _ => format!("{base}{}", t.url_suffix.as_deref().unwrap_or_default()),
            };
            e.force_stream = format == WireFormat::Gemini || t.force_stream == Some(true);
            e.headers = t.headers.clone().unwrap_or_default();
            e.auth = text_auth(t, format);
            if format == WireFormat::Claude && !e.headers.keys().any(|k| k.eq_ignore_ascii_case("anthropic-version")) {
                e.headers.insert("anthropic-version".into(), ANTHROPIC_VERSION.into());
            }
            e.timeout_ms = t.timeout_ms;
            e.stall_timeout_ms = t.stall_timeout_ms;
            e.vision = vision;
            e.retry = t
                .retry
                .iter()
                .flatten()
                .map(|(code, r)| {
                    let o = match r {
                        RetryPolicy::Attempts(n) => RetryOverride { retries: *n, delay_ms: 0 },
                        RetryPolicy::Policy { attempts, delay_ms } => {
                            RetryOverride { retries: *attempts, delay_ms: delay_ms.unwrap_or_default() }
                        }
                    };
                    (code.clone(), o)
                })
                .collect();
            e
        })
        .collect()
}

/// The API-key placement: declared, else 9router's default for the format.
fn text_auth(t: &Transport, format: WireFormat) -> Option<EndpointAuth> {
    let a = t.auth.as_ref();
    let declared = a.and_then(|a| a.api_key.as_ref().map(|k| (k.header.clone(), Some(k.scheme)))).or_else(|| {
        let a = a?;
        a.header.clone().map(|h| (h, a.scheme))
    });
    match declared {
        Some((header, scheme)) => {
            let bearer = header.eq_ignore_ascii_case("authorization");
            let scheme = scheme.unwrap_or(if bearer { AuthScheme::Bearer } else { AuthScheme::Raw });
            Some(EndpointAuth { header, scheme })
        }
        None if format == WireFormat::Claude => {
            Some(EndpointAuth { header: "x-api-key".into(), scheme: AuthScheme::Raw })
        }
        None => None,
    }
}

/// A capability section's endpoint, in the OpenAI format (the fit check refused the rest).
fn media_endpoint(e: &SectionEndpoint) -> Option<Endpoint> {
    let mut ep = endpoint(e.base_url.clone()?, Some("openai-chat"));
    if let Some(m) = &e.method {
        ep.method = m.clone();
    }
    ep.headers = e.headers.clone();
    ep.timeout_ms = e.timeout_ms;
    if e.auth_header.as_deref() == Some("x-api-key") {
        ep.auth = Some(EndpointAuth { header: "x-api-key".into(), scheme: AuthScheme::Raw });
    }
    Some(ep)
}
