//! The fit check (research R19, contracts/provider-schema-v2.md § Fit check). It runs after
//! the gate: *invalid* means malformed or unsafe, *unsupported* means well-formed but needing
//! something this core lacks. Any unsupported part refuses the whole plugin, and the message
//! lists every part.

use std::fmt;

use crate::schema::{
    AuthKind, AuthScheme, CapabilityKind, Category, ModelType, ProviderEntity, SectionEndpoint, Transport, WireFormat,
};
use crate::validate::gate::{GateCtx, positioned};
use crate::validate::{FieldPath, line_col};

/// The plugin schemas this core reads.
const SCHEMAS: &str = "1-2";

/// Whether this core can serve a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FitVerdict {
    Fits,
    Unsupported { parts: Vec<UnsupportedPart> },
}

/// One thing a plugin needs that this core lacks, positioned in its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedPart {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub path: FieldPath,
    /// The declared value as TOML, when it is a scalar or a short list.
    pub value: Option<String>,
    pub reason: String,
}

impl fmt::Display for UnsupportedPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{} {}", self.file, self.line, self.col, self.path)?;
        if let Some(v) = &self.value {
            write!(f, " = {v}")?;
        }
        write!(f, ": {}", self.reason)
    }
}

impl FitVerdict {
    pub fn fits(&self) -> bool {
        matches!(self, Self::Fits)
    }

    /// The refusal the operator sees, or `None` when the plugin fits.
    pub fn message(&self, file: &str) -> Option<String> {
        let Self::Unsupported { parts } = self else { return None };
        let mut out = format!(
            "{file}: not supported by this core (0router {}, plugin schema {SCHEMAS})\n",
            env!("CARGO_PKG_VERSION")
        );
        for p in parts {
            out.push_str(&format!("  - {p}\n"));
        }
        out.push_str("No part of this plugin was loaded.");
        Some(out)
    }
}

/// A schema newer than this core reads, found before the gate: a future schema may use keys
/// the gate would call unknown.
pub fn newer_schema(src: &str, file: &str) -> Option<FitVerdict> {
    let doc: toml::Table = toml::from_str(src).ok()?;
    let v = doc.get("schema")?.as_integer().filter(|v| *v > 2)?;
    let path = FieldPath::of("schema");
    let at = positioned(src, file, path.clone(), String::new());
    let reason = "this core reads plugin schemas 1 and 2".to_owned();
    let part =
        UnsupportedPart { file: file.to_owned(), line: at.line, col: at.col, path, value: Some(v.to_string()), reason };
    Some(FitVerdict::Unsupported { parts: vec![part] })
}

/// Checks a gated plugin. `src` and `file` position the parts.
pub fn check(p: &ProviderEntity, src: &str, file: &str, ctx: &GateCtx) -> FitVerdict {
    let mut found = Found { src, file, parts: Vec::new() };
    let f = &mut found;

    let sign_in = "account sign-in is not supported";
    let cookie = "cookie sign-in is not supported";
    match p.category {
        Category::Oauth => f.add(FieldPath::of("category"), q(p.category.as_str()), sign_in),
        Category::WebCookie => f.add(FieldPath::of("category"), q(p.category.as_str()), cookie),
        _ => {}
    }
    if let Some(a) = &p.auth {
        let why = |k: AuthKind| match k {
            AuthKind::Oauth => Some(sign_in),
            AuthKind::Cookie => Some(cookie),
            _ => None,
        };
        if let Some(r) = a.kind.and_then(why) {
            f.add(FieldPath::of("auth").key("kind"), q(a.kind.unwrap().as_str()), r);
        }
        for (i, k) in a.modes.iter().enumerate() {
            if let Some(r) = why(*k) {
                f.add(FieldPath::of("auth").key("modes").index(i), q(k.as_str()), r);
            }
        }
        if !a.hooks.is_empty() {
            f.add(FieldPath::of("auth").key("hooks"), list(a.hooks.iter().map(|h| h.as_str())), HOOK);
        }
        if let Some(x) = &a.credential_fallback {
            f.add(
                FieldPath::of("auth").key("credential_fallback"),
                q(x),
                "borrowing another provider's credential is not supported",
            );
        }
    }
    if p.oauth.is_some() {
        f.add(FieldPath::of("oauth"), None, sign_in);
    }
    // Slice 005: sign-in, identity, quota and live model lists are open to bundled plugins
    // only. Opening them up later is removing this rule.
    if !p.is_bundled() {
        let sections = [
            ("signin", p.signin.is_some(), sign_in),
            ("identity", p.identity.is_some(), sign_in),
            ("models_live", p.models_live.is_some(), sign_in),
            ("quota", p.quota.is_some(), "quota is not supported"),
        ];
        for (key, _, reason) in sections.into_iter().filter(|(_, present, _)| *present) {
            f.add(FieldPath::of(key), None, reason);
        }
    }
    for (i, r) in p.requires.iter().enumerate() {
        let reason = match r.split_once(':') {
            Some(("9router-executor", _)) => "needs a provider-specific executor".to_owned(),
            Some((need, _)) if need.starts_with("9router-") => {
                format!("needs a provider-specific {} handler", need.trim_start_matches("9router-"))
            }
            _ => format!("this core has no {r:?}"),
        };
        f.add(FieldPath::of("requires").index(i), q(r), &reason);
    }

    if let Some(t) = &p.transport {
        transport(f, t, FieldPath::of("transport"));
    }
    for (i, t) in p.transports.iter().enumerate() {
        transport(f, t, FieldPath::of("transports").index(i));
    }

    for (kind, sec) in &p.capabilities {
        // Schema 2 derives these entries from `endpoints`, which are checked below.
        if ModelType::from_capability(*kind).is_some_and(|t| p.endpoints.contains_key(&t)) {
            continue;
        }
        let base = FieldPath::of("capabilities").key(kind.as_str());
        match kind {
            CapabilityKind::Llm | CapabilityKind::ImageToText => {}
            CapabilityKind::WebSearch => f.add(base, None, "web search is not part of this core"),
            CapabilityKind::WebFetch => f.add(base, None, "web fetch is not part of this core"),
            CapabilityKind::Video if sec.endpoint.is_some() => {
                f.add(base.key("endpoint"), None, "video jobs need a schema 2 [endpoints.video] declaration")
            }
            k => match (ModelType::from_capability(*k), &sec.endpoint) {
                (None, _) => f.add(base, None, &format!("{k} is not part of this core")),
                (Some(t), None) => f.add(base, None, &format!("{t} served through the chat endpoint is not supported")),
                (Some(t), Some(e)) => media(f, t, e, base.key("endpoint"), ctx),
            },
        }
    }

    for (ty, eps) in &p.endpoints {
        for (i, e) in eps.0.iter().enumerate() {
            let Some(w) = &e.wire else { continue };
            if !ctx.style_ids.contains(w) {
                let base = FieldPath::of("endpoints").key(ty.as_str());
                let base = if eps.0.len() > 1 { base.index(i) } else { base };
                f.add(base.key("wire"), q(w), &format!("wire style {w:?} is not loaded"));
            }
        }
    }

    for (i, m) in p.models.iter().flatten().enumerate() {
        let base = FieldPath::of("models").index(i);
        if let Some(fm) = m.target_format.filter(|w| w.wire().is_none()) {
            f.add(base.key("target_format"), q(fm.as_str()), &unsupported_wire(fm));
        }
        if m.wires.is_none() {
            for (j, fm) in m.supported_formats.iter().flatten().enumerate().filter(|(_, w)| w.wire().is_none()) {
                f.add(base.key("supported_formats").index(j), q(fm.as_str()), &unsupported_wire(*fm));
            }
        }
        if m.strip.as_ref().is_some_and(|s| !s.is_empty()) {
            f.add(base.key("strip"), None, "stripping content kinds per model is not supported");
        }
    }

    // After web search and web fetch were dropped, some plugins declare nothing to call.
    let text = p.transport.iter().chain(&p.transports).any(|t| t.base_url.is_some() || t.base_urls.is_some());
    let media = p.capabilities.values().any(|s| s.endpoint.is_some());
    if f.parts.is_empty() && !text && !media && p.endpoints.is_empty() {
        f.add(FieldPath::of("id"), q(&p.id), "declares no model type this core serves");
    }

    if found.parts.is_empty() { FitVerdict::Fits } else { FitVerdict::Unsupported { parts: found.parts } }
}

const HOOK: &str = "auth hooks need provider-specific code";

fn unsupported_wire(f: WireFormat) -> String {
    format!("wire format {:?} is not supported", f.as_str())
}

fn transport(f: &mut Found, t: &Transport, base: FieldPath) {
    if let Some(fm) = t.format.filter(|w| w.wire().is_none()) {
        f.add(base.key("format"), q(fm.as_str()), &unsupported_wire(fm));
    }
    let urls = t.base_url.iter().chain(t.base_urls.iter().flatten());
    if urls.into_iter().any(|u| u.contains('{')) {
        let key = if t.base_url.is_some() { "base_url" } else { "base_urls" };
        f.add(base.key(key), None, "a URL filled from per-account data is not supported");
    }
    if let Some(tf) = t.thinking_format.as_deref().filter(|tf| *tf != "openai") {
        f.add(base.key("thinking_format"), q(tf), "a provider-specific reasoning format is not supported");
    }
    if t.reasoning_inject.is_some() {
        f.add(base.key("reasoning_inject"), None, "injecting reasoning into requests needs provider-specific code");
    }
    for (key, v) in
        [("chat_path", &t.chat_path), ("responses_url", &t.responses_url), ("messages_url", &t.messages_url)]
    {
        if let Some(v) = v {
            f.add(base.key(key), q(v), "routing a request to a second URL by format is not supported");
        }
    }
    if !t.quirks.is_empty() {
        f.add(
            base.key("quirks"),
            list(t.quirks.iter().map(|q| q.as_str())),
            "provider quirks need provider-specific code",
        );
    }
    for (key, v) in [
        ("claude_supported_tool_types", &t.claude_supported_tool_types),
        ("force_auto_tool_choice_models", &t.force_auto_tool_choice_models),
    ] {
        if v.is_some() {
            f.add(base.key(key), None, "provider quirks need provider-specific code");
        }
    }
    if t.executor_params.is_some() {
        f.add(base.key("executor_params"), None, "executor parameters need a provider-specific executor");
    }
    if let Some(r) = &t.default_region {
        f.add(base.key("default_region"), q(r), "regions are not supported");
    }
    if t.regions.is_some() {
        f.add(base.key("regions"), None, "regions are not supported");
    }
    let schemes = t.auth.iter().flat_map(|a| a.scheme.into_iter().chain(a.api_key.as_ref().map(|k| k.scheme)));
    if let Some(s) = schemes.into_iter().find(|s| !matches!(s, AuthScheme::Bearer | AuthScheme::Raw)) {
        f.add(base.key("auth"), None, &format!("auth scheme {:?} is not supported", s.as_str()));
    }
    if t.auth.as_ref().is_some_and(|a| !a.hooks.is_empty()) {
        let hooks = t.auth.as_ref().unwrap().hooks.iter().map(|h| h.as_str());
        f.add(base.key("auth").key("hooks"), list(hooks), HOOK);
    }
}

fn media(f: &mut Found, t: ModelType, e: &SectionEndpoint, base: FieldPath, ctx: &GateCtx) {
    match e.format.as_deref() {
        None | Some("openai") => {
            if !ctx.style_types.get("openai-chat").is_some_and(|ts| ts.contains(&t)) {
                f.add(base.clone(), None, &format!("no loaded style carries {t} in the OpenAI format"));
            }
        }
        Some(fm) => f.add(base.key("format"), q(fm), &format!("media format {fm:?} is not implemented")),
    }
    for (key, present) in [
        ("model_map", e.model_map.is_some()),
        ("poll_url", e.poll_url.is_some()),
        ("body_fields", e.body_fields.is_some()),
    ] {
        if present {
            f.add(base.key(key), None, "a provider-specific request shape is not supported");
        }
    }
    if let Some(h) = e.auth_header.as_deref().filter(|h| !is_plain_auth(h)) {
        f.add(base.key("auth_header"), q(h), "this way of sending the key is not supported");
    }
    if e.via_chat {
        f.add(base.key("via_chat"), None, "answering through the chat endpoint is not supported");
    }
}

/// The section `auth_header` values schema 2 can express: `Authorization: Bearer`, or the
/// key as is in `x-api-key`. 9router's `token`, `key`, `basic`, … prefixes and signatures
/// are not.
pub(crate) fn is_plain_auth(h: &str) -> bool {
    matches!(h, "bearer" | "authorization" | "x-api-key" | "none")
}

fn q(s: &str) -> Option<String> {
    Some(format!("{s:?}"))
}

fn list<'a>(items: impl Iterator<Item = &'a str>) -> Option<String> {
    Some(format!("[{}]", items.map(|s| format!("{s:?}")).collect::<Vec<_>>().join(", ")))
}

struct Found<'a> {
    src: &'a str,
    file: &'a str,
    parts: Vec<UnsupportedPart>,
}

impl Found<'_> {
    fn add(&mut self, path: FieldPath, value: Option<String>, reason: &str) {
        let at = positioned(self.src, self.file, path.clone(), String::new());
        let (line, col) = if at.line == 0 { line_col(self.src, 0) } else { (at.line, at.col) };
        self.parts.push(UnsupportedPart {
            file: self.file.to_owned(),
            line,
            col,
            path,
            value,
            reason: reason.to_owned(),
        });
    }
}
