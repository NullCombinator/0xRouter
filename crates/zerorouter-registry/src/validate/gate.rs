//! The validation gate (FR-007 – FR-010). Bundled and user plugins pass the same checks.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::errors::{FieldPath, Seg, ValidationError, line_col};
use super::secrets::{check_map_key, check_query, check_url};
use super::ssrf::check_endpoint_url;
use super::style_gate::placeholders;
use crate::floor::{Floor, PatternRisk};
use crate::schema::{
    AuthScheme, CapabilityKind, Endpoint, Forwarding, KNOWN_OAUTH_PARAMS, KNOWN_SECTION_FORMATS, ModelType, PluginFile,
    PluginSource, ProviderEntity, RouteOp, Transport,
};
use crate::template::{FieldPath as Selector, Template};

/// What the plugin gate knows about the rest of the load.
#[derive(Debug, Clone, Default)]
pub struct GateCtx {
    /// Loaded style ids. An unknown wire is left to the fit check.
    pub style_ids: BTreeSet<String>,
    /// Route ops each loaded style serves.
    pub style_ops: BTreeMap<String, BTreeSet<RouteOp>>,
    /// Model types each loaded style has a codec for.
    pub style_types: BTreeMap<String, BTreeSet<ModelType>>,
    /// Every loaded style's access-key carrier headers (security floor).
    pub style_carriers: Vec<String>,
    /// Bundled plugins and CI: forwarding a floor name is an error, not a diagnostic.
    pub strict: bool,
    /// Operator's `allow_private_endpoints`.
    pub allow_private: bool,
}

/// A plugin that passed the gate, with its non-fatal diagnostics.
#[derive(Debug, Clone)]
pub struct Gated {
    pub entity: ProviderEntity,
    /// Warnings and stripped forwarding entries.
    pub diagnostics: Vec<ValidationError>,
}

/// Parses and checks one plugin file. Returns every error found, not just the first.
pub fn validate(src: &str, source: PluginSource, file: &str) -> Result<ProviderEntity, Vec<ValidationError>> {
    validate_with(src, source, file, &GateCtx::default()).map(|g| g.entity)
}

/// [`validate`] with the load context schema 2 needs.
pub fn validate_with(src: &str, source: PluginSource, file: &str, ctx: &GateCtx) -> Result<Gated, Vec<ValidationError>> {
    let mut plugin = parse::<PluginFile>(src, file)?;
    let mut errors = semantic_errors(&plugin);
    let mut diags = Found::new();
    if plugin.schema_version() >= 2 {
        schema2_errors(&mut plugin, ctx, &mut errors, &mut diags);
    }
    let at = |v: Found| v.into_iter().map(|(path, rule)| positioned(src, file, path, rule)).collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(Gated { entity: ProviderEntity::from_file(plugin, source), diagnostics: at(diags) })
    } else {
        Err(at(errors))
    }
}

/// Deserialises `src` with path-and-span error reporting. Shared with the config loader.
pub(crate) fn parse<T: serde::de::DeserializeOwned>(src: &str, file: &str) -> Result<T, Vec<ValidationError>> {
    let de = toml::de::Deserializer::parse(src).map_err(|e| vec![toml_error(src, file, FieldPath::root(), &e)])?;
    serde_path_to_error::deserialize(de).map_err(|e| {
        let path = FieldPath(
            e.path()
                .iter()
                .filter_map(|s| match s {
                    serde_path_to_error::Segment::Seq { index } => Some(Seg::Index(*index)),
                    serde_path_to_error::Segment::Map { key } => Some(Seg::Key(key.clone())),
                    serde_path_to_error::Segment::Enum { variant } => Some(Seg::Key(variant.clone())),
                    serde_path_to_error::Segment::Unknown => None,
                })
                .collect(),
        );
        vec![toml_error(src, file, path, e.inner())]
    })
}

fn toml_error(src: &str, file: &str, path: FieldPath, e: &toml::de::Error) -> ValidationError {
    let (line, col) = e.span().map_or((0, 0), |s| line_col(src, s.start));
    ValidationError { file: file.to_owned(), line, col, path, rule: e.message().trim().to_owned() }
}

/// An error at `path`, positioned by walking the parsed document.
pub(crate) fn positioned(src: &str, file: &str, path: FieldPath, rule: String) -> ValidationError {
    let (line, col) = locate(src, &path).map_or((0, 0), |s| line_col(src, s.start));
    ValidationError { file: file.to_owned(), line, col, path, rule }
}

/// The span of the deepest existing prefix of `path`: the key for a table entry, the
/// value for an array element.
fn locate(src: &str, path: &FieldPath) -> Option<Range<usize>> {
    let root = DeTable::parse(src).ok()?;
    let mut span = None;
    let mut table: Option<&DeTable<'_>> = Some(root.get_ref());
    let mut array: Option<&[Spanned<DeValue<'_>>]> = None;
    for seg in &path.0 {
        let value = match (seg, table, array) {
            (Seg::Key(k), Some(t), _) => {
                let (key, value) = t.get_key_value(k.as_str())?;
                span = Some(key.span());
                value
            }
            (Seg::Index(i), _, Some(a)) => {
                let value = a.get(*i)?;
                span = Some(value.span());
                value
            }
            _ => break,
        };
        table = value.get_ref().as_table();
        array = value.get_ref().as_array().map(|a| &a[..]);
    }
    span
}

type Found = Vec<(FieldPath, String)>;

fn semantic_errors(p: &PluginFile) -> Found {
    let mut out = Found::new();
    let mut err = |path: FieldPath, rule: String| out.push((path, rule));

    if let Some(v) = p.schema.filter(|v| !(1..=2).contains(v)) {
        err(FieldPath::of("schema"), format!("unsupported schema version {v}; expected 1 or 2"));
    }
    if p.schema_version() < 2 {
        for key in ["endpoints", "forwarding", "session"] {
            let present = match key {
                "endpoints" => !p.endpoints.is_empty(),
                "forwarding" => p.forwarding.is_some(),
                _ => p.session.is_some(),
            };
            if present {
                err(FieldPath::of(key), "schema 1 has no `".to_owned() + key + "`; set schema = 2");
            }
        }
        for (i, _) in p.models.iter().flatten().enumerate().filter(|(_, m)| m.wires.is_some()) {
            err(FieldPath::of("models").index(i).key("wires"), "schema 1 has no `wires`; set schema = 2".into());
        }
    }
    if !is_token(&p.id) {
        err(FieldPath::of("id"), format!("{:?} must match [a-z0-9][a-z0-9-]*", p.id));
    }
    if let Some(a) = p.alias.as_deref().filter(|a| !is_token(a)) {
        err(FieldPath::of("alias"), format!("{a:?} must match [a-z0-9][a-z0-9-]*"));
    }

    if let Some(t) = &p.transport {
        check_transport(t, &FieldPath::of("transport"), &mut err);
    }
    for (i, t) in p.transports.iter().enumerate() {
        check_transport(t, &FieldPath::of("transports").index(i), &mut err);
    }
    if !p.transports.is_empty() && p.transport.is_none() {
        err(FieldPath::of("transports"), "requires [transport] to be set".into());
    }

    if let Some(o) = &p.oauth {
        let base = FieldPath::of("oauth");
        for (k, url) in o.url_fields() {
            if let Err(rule) = check_url(url) {
                err(base.key(k), rule.into());
            }
        }
        for k in o.params.keys().filter(|k| !KNOWN_OAUTH_PARAMS.contains(&k.as_str())) {
            err(
                base.key("params").key(k.as_str()),
                format!("unknown OAuth parameter; allowed: {}", KNOWN_OAUTH_PARAMS.join(", ")),
            );
        }
    }

    for (kind, section) in &p.capabilities {
        let base = FieldPath::of("capabilities").key(kind.as_str());
        match &section.endpoint {
            None if p.transport.is_none() => err(base, "no endpoint and provider has no transport".into()),
            None => {}
            Some(e) => {
                let base = base.key("endpoint");
                for (k, url) in e.url_fields() {
                    if let Err(rule) = check_url(url) {
                        err(base.key(k), rule.into());
                    }
                }
                check_headers(e.headers.keys(), &base.key("headers"), &mut err);
                if let Some(f) = e.format.as_deref().filter(|f| !KNOWN_SECTION_FORMATS.contains(f)) {
                    err(
                        base.key("format"),
                        format!("unknown format {f:?}; allowed: {}", KNOWN_SECTION_FORMATS.join(", ")),
                    );
                }
            }
        }
    }

    // One id may appear once per kind: gemini-2.5-pro is both an llm and an stt model.
    // Lookup by id returns the first entry, as 9router's `findModel` does.
    let mut seen: HashMap<(&str, Option<CapabilityKind>), usize> = HashMap::new();
    for (i, m) in p.models.iter().flatten().enumerate() {
        let base = FieldPath::of("models").index(i);
        if let Some(&j) = seen.get(&(m.id.as_str(), m.kind)) {
            err(base.key("id"), format!("duplicate of models[{j}]"));
        } else {
            seen.insert((&m.id, m.kind), i);
        }
        for (j, name) in m.params.iter().flatten().enumerate() {
            if check_map_key(name).is_some() {
                err(base.key("params").index(j), format!("secret-like parameter {name:?} not allowed in plugins"));
            }
        }
    }
    out
}

fn check_transport(t: &Transport, base: &FieldPath, err: &mut impl FnMut(FieldPath, String)) {
    for (k, url) in t.url_fields() {
        // An empty base URL means the operator supplies it (azure).
        if k == "base_url" && url.is_empty() {
            continue;
        }
        if let Err(rule) = check_url(url) {
            err(base.key(k), rule.into());
        }
    }
    for (k, v) in [("url_suffix", &t.url_suffix), ("chat_path", &t.chat_path)] {
        if let Some(Err(rule)) = v.as_deref().map(check_query) {
            err(base.key(k), rule.into());
        }
    }
    check_headers(t.headers.iter().flat_map(|h| h.keys()), &base.key("headers"), err);
    if let Some(r) = &t.default_region {
        if !t.regions.as_ref().is_some_and(|m| m.contains_key(r)) {
            err(base.key("default_region"), format!("{r:?} is not a key of regions"));
        }
    }
}

fn check_headers<'a>(
    names: impl Iterator<Item = &'a String>,
    base: &FieldPath,
    err: &mut impl FnMut(FieldPath, String),
) {
    for name in names.filter(|n| check_map_key(n).is_some()) {
        err(base.key(name.as_str()), "credential-bearing header not allowed in plugins".into());
    }
}

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];

/// Schema-2 rules: endpoints, forwarding and session. Floor names in forwarding lists are
/// stripped from `p` with a diagnostic (an error under `strict`).
fn schema2_errors(p: &mut PluginFile, ctx: &GateCtx, errors: &mut Found, diags: &mut Found) {
    let mut err = |path: FieldPath, rule: String| errors.push((path, rule));
    for key in p.schema1_execution_keys() {
        let path = key.split('.').fold(FieldPath::root(), |fp, k| fp.key(k));
        err(path, "schema 2 declares this under `endpoints`".into());
    }

    for (t, eps) in &p.endpoints {
        for (i, e) in eps.0.iter().enumerate() {
            let base = FieldPath::of("endpoints").key(t.as_str());
            let base = if eps.0.len() > 1 { base.index(i) } else { base };
            check_endpoint(e, *t, &base, ctx, &mut err);
        }
    }

    for (i, m) in p.models.iter().flatten().enumerate() {
        let Some(t) = m.kind.and_then(ModelType::from_capability) else { continue };
        if !p.endpoints.contains_key(&t) {
            err(
                FieldPath::of("models").index(i).key("kind"),
                format!("model-type-without-endpoint: no [endpoints.{t}] for this {t} model"),
            );
        }
    }

    if let Some(s) = &p.session
        && !s.header.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        err(FieldPath::of("session").key("header"), format!("{:?} is not a header name", s.header));
    }

    let own_auth: Vec<String> = p.auth_headers().map(str::to_owned).collect();
    let floor = Floor::computed(ctx.style_carriers.iter().map(String::as_str), own_auth.iter().map(String::as_str));
    if let Some(f) = p.forwarding.as_mut() {
        check_forwarding(f, &floor, ctx.strict, &mut err, &mut |path, rule| diags.push((path, rule)));
    }
}

fn check_endpoint(e: &Endpoint, t: ModelType, base: &FieldPath, ctx: &GateCtx, err: &mut impl FnMut(FieldPath, String)) {
    if let Err(rule) = check_endpoint_url(&e.url, ctx.allow_private) {
        err(base.key("url"), rule);
    }
    if !METHODS.contains(&e.method.as_str()) {
        err(base.key("method"), format!("{:?} is not one of {}", e.method, METHODS.join(", ")));
    }
    if let Some(a) = &e.auth {
        if a.header.is_empty() || !a.header.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            err(base.key("auth").key("header"), format!("{:?} is not a header name", a.header));
        }
        if !matches!(a.scheme, AuthScheme::Bearer | AuthScheme::Raw) {
            err(base.key("auth").key("scheme"), format!("{:?}: an endpoint takes `bearer` or `raw`", a.scheme.as_str()));
        }
    }
    match (&e.wire, &e.body, &e.response) {
        (Some(_), None, None) | (None, Some(_), Some(_)) => {}
        (Some(_), _, _) => err(base.key("wire"), "`wire` and inline `body`/`response` are mutually exclusive".into()),
        (None, None, None) => err(base.clone(), "an endpoint needs `wire` or inline `body` + `response`".into()),
        (None, _, _) => err(base.clone(), "inline endpoints need both `body` and `response`".into()),
    }
    if let Some(body) = &e.body {
        match Template::parse(body) {
            Err(te) => err(base.key("body"), te.to_string()),
            Ok(tpl) => {
                if let Err(te) = tpl.check(placeholders::PROVIDER_BODY) {
                    err(base.key("body"), te.to_string());
                }
            }
        }
        if let Some(k) = secret_key(body) {
            err(base.key("body").key(k.as_str()), "secret-like body field not allowed in plugins".into());
        }
    }
    if let Some(resp) = &e.response {
        match resp.as_table() {
            None => err(base.key("response"), "maps IR fields to response field paths".into()),
            Some(tbl) => {
                for (k, v) in tbl {
                    match v.as_str().map(Selector::parse) {
                        Some(Ok(_)) => {}
                        Some(Err(rule)) => err(base.key("response").key(k.as_str()), rule),
                        None => err(base.key("response").key(k.as_str()), "must be a field path string".into()),
                    }
                }
            }
        }
    }
    if e.encoding.is_some() && e.body.is_none() {
        err(base.key("encoding"), "`encoding` applies to inline bodies only".into());
    }
    for name in e.headers.keys().filter(|n| check_map_key(n).is_some()) {
        err(base.key("headers").key(name.as_str()), "credential-bearing header not allowed in plugins".into());
    }
    for status in e.retry.keys().filter(|s| !is_status(s)) {
        err(base.key("retry").key(status.as_str()), "keys are HTTP statuses 100-599".into());
    }
    for (kind, rules) in [("body", &e.errors.body), ("stream", &e.errors.stream)] {
        for (i, r) in rules.iter().enumerate() {
            let rb = base.key("errors").key(kind).index(i);
            for (k, path) in [("message", Some(&r.message)), ("status", r.status.as_ref())] {
                if let Some(Err(rule)) = path.map(|p| Selector::parse(p)) {
                    err(rb.key(k), rule);
                }
            }
            let bad = r.status_map.values().chain(&r.code).any(|s| !(100..=599).contains(s));
            if bad {
                err(rb.clone(), "error-rule statuses must be 100-599".into());
            }
        }
    }
    if let Some(tc) = &e.token_count {
        if let Err(rule) = check_endpoint_url(&tc.url, ctx.allow_private) {
            err(base.key("token_count").key("url"), rule);
        }
        let counts = t == ModelType::Text
            && e.wire.as_ref().is_some_and(|w| ctx.style_ops.get(w).is_some_and(|ops| ops.contains(&RouteOp::CountTokens)));
        if !counts {
            err(base.key("token_count"), "token_count needs a wire style with a count_tokens route".into());
        }
    }
    if let Some(c) = &e.continuation
        && !c.models.is_empty()
        && !c.except_models.is_empty()
    {
        err(base.key("continuation"), "set `models` or `except_models`, not both".into());
    }
    if t != ModelType::Text && (e.continuation.is_some() || e.vision) {
        err(base.clone(), "`continuation` and `vision` apply to text endpoints only".into());
    }
}

fn check_forwarding(
    f: &mut Forwarding,
    floor: &Floor,
    strict: bool,
    err: &mut impl FnMut(FieldPath, String),
    diag: &mut impl FnMut(FieldPath, String),
) {
    let mut check = |name: &str, path: FieldPath| -> bool {
        if name.contains('*') {
            match floor.pattern_risk(name) {
                PatternRisk::Ok => true,
                PatternRisk::Warning(w) => {
                    diag(path, format!("warning: {w}"));
                    true
                }
                PatternRisk::Error(e) => {
                    err(path, e);
                    true
                }
            }
        } else if floor.blocks(name) {
            let rule = format!("{name:?} is in the security floor and is never forwarded");
            if strict {
                err(path, rule);
                true
            } else {
                diag(path, format!("{rule}; entry stripped"));
                false
            }
        } else {
            true
        }
    };
    let up = FieldPath::of("forwarding").key("to_upstream").key("headers");
    let mut i = 0;
    f.to_upstream.headers.retain(|h| {
        let keep = check(&h.name, up.index(i).key("name"));
        i += 1;
        keep
    });
    let down = FieldPath::of("forwarding").key("to_client").key("headers");
    let mut i = 0;
    f.to_client.headers.retain(|h| {
        let keep = check(h, down.index(i));
        i += 1;
        keep
    });
    for (i, path) in f.to_client.body.iter().enumerate() {
        let at = FieldPath::of("forwarding").key("to_client").key("body").index(i);
        match Selector::parse(path) {
            Err(rule) => err(at, rule),
            Ok(sel) => {
                let secret = sel.0.iter().any(|s| matches!(s, crate::template::PathSeg::Key(k) if is_secret_name(k)));
                if secret {
                    err(at, "secret-like body path not allowed in forwarding".into());
                }
            }
        }
    }
}

/// A secret-like key anywhere in an inline body template.
fn secret_key(v: &toml::Value) -> Option<String> {
    match v {
        toml::Value::Table(t) => t
            .iter()
            .find_map(|(k, v)| if is_secret_name(k) { Some(k.clone()) } else { secret_key(v) }),
        toml::Value::Array(a) => a.iter().find_map(secret_key),
        _ => None,
    }
}

/// Slice 002's secret-name check, except token *counts* (`max_tokens`, `input_tokens`).
fn is_secret_name(k: &str) -> bool {
    check_map_key(k).is_some_and(|term| term != "token" || !k.to_ascii_lowercase().ends_with("tokens"))
}

fn is_status(s: &str) -> bool {
    s.parse::<u16>().is_ok_and(|n| (100..=599).contains(&n))
}

/// `[a-z0-9][a-z0-9-]*`
fn is_token(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(src: &str) -> Vec<String> {
        validate(src, PluginSource::Bundled, "t.toml").unwrap_err().iter().map(ToString::to_string).collect()
    }

    #[test]
    fn minimal_is_valid() {
        let e = validate("schema = 1\nid = \"p\"\ncategory = \"apikey\"\n", PluginSource::Bundled, "t.toml").unwrap();
        assert!(e.models.is_none() && e.transport.is_none());
    }

    #[test]
    fn unknown_field_has_path_and_position() {
        let e = errors("id = \"p\"\ncategory = \"apikey\"\n[oauth]\nclient_secret = \"x\"\n");
        assert_eq!(e.len(), 1);
        assert!(e[0].starts_with("t.toml:4:1 oauth"), "{}", e[0]);
        assert!(e[0].contains("client_secret"), "{}", e[0]);
    }

    #[test]
    fn semantic_errors_are_collected_and_positioned() {
        let e = errors(
            "id = \"P\"\ncategory = \"apikey\"\n[transport]\nheaders = { Authorization = \"x\" }\n\
             [[models]]\nid = \"a\"\n[[models]]\nid = \"a\"\n",
        );
        assert_eq!(e.len(), 3, "{e:#?}");
        assert!(e[0].starts_with("t.toml:1:1 id:"), "{}", e[0]);
        assert!(e[1].starts_with("t.toml:4:13 transport.headers.Authorization: credential-bearing"), "{}", e[1]);
        assert!(e[2].contains("models[1].id: duplicate of models[0]"), "{}", e[2]);
    }

    const V2: &str = r#"schema = 2
id = "acme"
category = "apikey"

[auth]
kind = "apikey"
header = "x-acme-key"

[endpoints.text]
url = "https://api.acme.example/v1/messages"
wire = "anthropic-messages"

[endpoints.text.token_count]
url = "https://api.acme.example/v1/messages/count_tokens"

[endpoints.stt]
url = "https://api.acme.example/v1/speech-to-text"
encoding = "multipart"
body = { file = "{input.audio}", model_id = "{model.upstream_id}" }
response = { text = "text" }

[forwarding.to_upstream]
headers = [{ name = "anthropic-beta", merge = "append_csv", from_styles = ["anthropic-messages"] }]

[forwarding.to_client]
headers = ["request-id", "anthropic-ratelimit-*"]

[[models]]
id = "m1"
kind = "llm"
"#;

    fn ctx() -> GateCtx {
        GateCtx {
            style_ids: ["anthropic-messages".to_owned()].into(),
            style_ops: [("anthropic-messages".to_owned(), [RouteOp::Generate, RouteOp::CountTokens].into())].into(),
            style_carriers: vec!["x-api-key".into()],
            ..GateCtx::default()
        }
    }

    fn v2(src: &str, ctx: &GateCtx) -> Result<Gated, Vec<String>> {
        validate_with(src, PluginSource::Bundled, "t.toml", ctx).map_err(|e| e.iter().map(ToString::to_string).collect())
    }

    fn v2_fails(src: &str, want: &str) {
        let got = v2(src, &ctx()).err().unwrap_or_default();
        assert!(got.iter().any(|r| r.contains(want)), "want {want:?} in {got:#?}");
    }

    #[test]
    fn schema2_valid() {
        let g = v2(V2, &ctx()).unwrap();
        assert!(g.diagnostics.is_empty(), "{:?}", g.diagnostics);
        assert_eq!(g.entity.endpoints[&ModelType::Text].0[0].wire.as_deref(), Some("anthropic-messages"));
    }

    #[test]
    fn schema2_rules() {
        v2_fails(&V2.replace("api.acme.example/v1/messages\"", "127.0.0.1/v1/messages\""), "endpoints.text.url: host 127.0.0.1");
        v2_fails(&V2.replace("https://api.acme.example/v1/speech", "https://{model}.acme.example/v1/speech"), "only in the URL path");
        v2_fails(&V2.replace("wire = \"anthropic-messages\"", "wire = \"anthropic-messages\"\nbody = {}"), "mutually exclusive");
        v2_fails(&V2.replace("{input.audio}", "{secret.key}"), "never see accounts or secrets");
        v2_fails(&V2.replace("{input.audio}", "{request.x}"), "unknown placeholder {request.x}");
        v2_fails(&V2.replace("file = ", "api_key = \"x\", file = "), "endpoints.stt.body.api_key: secret-like body field");
        v2_fails(&V2.replace("\"request-id\", ", "\"*\", "), "a wildcard needs a prefix");
        v2_fails(&V2.replace("merge = \"append_csv\"", "merge = \"prepend\""), "unknown forward merge");
        v2_fails(&format!("{V2}\n[endpoints.tts]\nurl = \"https://a.example/x\"\nwire = \"anthropic-messages\"\ncontinuation = {{ method = \"guess\" }}\n"), "unknown continuation method");
        v2_fails(&V2.replace("url = \"https://api.acme.example/v1/messages\"\n", "url = \"https://api.acme.example/v1/messages\"\nerrors = { body = [{ message = \"e\", code = 99 }] }\n"), "statuses must be 100-599");
        v2_fails(&format!("{V2}[transport]\nbase_url = \"https://a.example\"\n"), "transport: schema 2 declares this under `endpoints`");
        v2_fails(&V2.replace("kind = \"llm\"", "kind = \"image\""), "model-type-without-endpoint");
        let no_count = GateCtx { style_ops: BTreeMap::new(), ..ctx() };
        let got = v2(V2, &no_count).unwrap_err();
        assert!(got.iter().any(|r| r.contains("count_tokens route")), "{got:?}");
    }

    #[test]
    fn floor_names_stripped_or_strict_error() {
        let src = V2.replace("\"request-id\", ", "\"request-id\", \"set-cookie\", ");
        let g = v2(&src, &ctx()).unwrap();
        assert_eq!(g.entity.forwarding.as_ref().unwrap().to_client.headers, vec!["request-id", "anthropic-ratelimit-*"]);
        assert!(g.diagnostics[0].to_string().contains("entry stripped"), "{:?}", g.diagnostics);
        let strict = GateCtx { strict: true, ..ctx() };
        assert!(v2(&src, &strict).unwrap_err()[0].contains("security floor"));
        let warn = v2(&V2.replace("\"request-id\", ", "\"x-api-*\", "), &ctx()).unwrap();
        assert!(warn.diagnostics[0].to_string().contains("warning:"), "{:?}", warn.diagnostics);
    }

    #[test]
    fn schema1_rejects_schema2_keys() {
        let got = errors("schema = 1\nid = \"p\"\ncategory = \"apikey\"\n[endpoints.text]\nurl = \"https://a.example\"\nwire = \"x\"\n");
        assert!(got.iter().any(|r| r.contains("schema 1 has no `endpoints`")), "{got:?}");
    }
}
