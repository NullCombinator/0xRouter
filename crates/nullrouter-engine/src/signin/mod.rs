//! Account sign-in (research R3, R4, R9, R10): the generic flows, token refresh, refresh
//! failure classification and per-account refresh dedup.
//!
//! The flows run wherever the core runs them (the CLI, research R2) and never touch a
//! terminal or a browser: [`begin`] returns a [`Begun`] flow, the caller shows its link or
//! code, and the flow completes to a [`TokenEntry`] the caller writes. Every call goes
//! through the engine's SSRF-checking, redirect-free client.
//!
//! ```text
//! begin ─► Begun::Device(d) ─► show d.link(), d.user_code ─► d.poll(..) ─► TokenEntry
//!      └─► Begun::Pkce(s) ──► show s.authorize_url() ─► s.wait_code(paste, ..) ─► s.complete(..)
//! ```
//!
//! Nothing here logs or returns a code, verifier or token; errors carry the provider's
//! error code and HTTP status only.

pub mod classify;
pub mod dedup;
pub mod device_code;
pub mod loopback;
pub mod pkce;
pub mod refresh;

use std::collections::BTreeSet;
use std::future::Future;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use nullrouter_registry::SecretString;
use nullrouter_registry::schema::{
    AuthScheme, EndpointAuth, ProviderEntity, RedirectKind, SignInDecl, SignInFlow, SignInProfile, TokenBody, ValuePath,
};
use reqwest::Url;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub use device_code::{DevicePoll, DeviceSignIn};
pub use loopback::Loopback;
pub use pkce::PasteError;

use crate::tokens::{Claims, TokenEntry};
use crate::upstream;

/// How long a browser sign-in waits for the redirect or a paste (research R3).
pub const BROWSER_TIMEOUT: Duration = Duration::from_secs(600);
/// Each sign-in HTTP call's limit.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// The access-token lifetime assumed when a token response has no `expires_in`.
pub const DEFAULT_EXPIRES_IN: Duration = Duration::from_secs(3600);

/// The client sign-in calls go through: the engine's redirect-free client with the SSRF
/// resolver, plus the address-literal check.
#[derive(Debug, Clone)]
pub struct SignInHttp {
    client: reqwest::Client,
    allow_private: bool,
    timeout: Duration,
}

impl SignInHttp {
    /// A new client; `allow_private` is the operator's `allow_private_endpoints`.
    pub fn new(allow_private: bool) -> Self {
        Self::with_client(upstream::client(allow_private), allow_private)
    }

    /// Shares a client built by [`upstream::client`] with the same `allow_private`.
    pub fn with_client(client: reqwest::Client, allow_private: bool) -> Self {
        Self { client, allow_private, timeout: HTTP_TIMEOUT }
    }

    /// Sets each call's limit.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// `raw` parsed and checked: http(s), and no private address literal unless allowed.
    pub fn url(&self, raw: &str) -> Result<Url, SignInError> {
        let bad = |reason: String| SignInError::BadUrl { url: raw.to_owned(), reason };
        let u = Url::parse(raw).map_err(|e| bad(e.to_string()))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err(bad("not an http(s) URL".into()));
        }
        upstream::check_ip_host(&u, self.allow_private).map_err(|e| bad(e.to_string()))?;
        Ok(u)
    }

    /// Sends a request built from `url`; returns the status and body.
    pub(crate) async fn send(
        &self,
        what: &'static str,
        method: reqwest::Method,
        url: &str,
        headers: HeaderMap,
        body: Option<(&'static str, String)>,
    ) -> Result<(u16, Bytes), SignInError> {
        let url = self.url(url)?;
        let mut rb = self.client.request(method, url).headers(headers).header(ACCEPT, "application/json");
        if let Some((ct, body)) = body {
            rb = rb.header(CONTENT_TYPE, ct).body(body);
        }
        let transport = |e: reqwest::Error| SignInError::Transport { what, reason: transport_reason(&e, self.timeout) };
        let r = rb.timeout(self.timeout).send().await.map_err(transport)?;
        let status = r.status().as_u16();
        let body = r.bytes().await.map_err(transport)?;
        Ok((status, body))
    }
}

fn transport_reason(e: &reqwest::Error, limit: Duration) -> String {
    if e.is_timeout() {
        return format!("timed out after {} ms", limit.as_millis());
    }
    let mut out = if e.is_connect() { "connection failed".to_owned() } else { "request failed".to_owned() };
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        out.push_str(": ");
        out.push_str(&s.to_string());
        src = s.source();
    }
    out
}

/// Why a sign-in ended. Never holds a code, verifier or token.
#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    #[error("provider {0} has no [signin] section")]
    NotSignIn(String),
    #[error("{url}: {reason}")]
    BadUrl { url: String, reason: String },
    #[error("{what}: {reason}")]
    Transport { what: &'static str, reason: String },
    #[error("{what} answered {status}: {code}")]
    Rejected { what: &'static str, status: u16, code: String },
    #[error("{what}: {reason}")]
    BadResponse { what: &'static str, reason: String },
    #[error("the sign-in was refused: {0}")]
    Denied(String),
    #[error("the device code expired before the sign-in was approved")]
    Expired,
    #[error("no sign-in within {} s", .0.as_secs())]
    TimedOut(Duration),
    #[error("sign-in cancelled")]
    Cancelled,
    #[error(transparent)]
    Paste(#[from] PasteError),
    #[error("nothing left to wait for: no loopback listener and no more input")]
    NoInput,
}

impl SignInError {
    /// Timeout, connection failure, 5xx or 429 (research R10).
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Transport { .. } => true,
            Self::Rejected { status, .. } => *status >= 500 || *status == 429,
            _ => false,
        }
    }
}

/// What a token endpoint granted.
#[derive(Debug)]
pub struct TokenGrant {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_in: Option<Duration>,
    pub scope: Option<String>,
    /// Read for display claims only, then dropped.
    pub id_token: Option<SecretString>,
}

/// A token request body: content type and encoded fields.
pub fn encode_body(kind: TokenBody, fields: &[(&str, &str)]) -> (&'static str, String) {
    match kind {
        TokenBody::Form => (
            "application/x-www-form-urlencoded",
            url::form_urlencoded::Serializer::new(String::new()).extend_pairs(fields).finish(),
        ),
        TokenBody::Json => {
            let map: serde_json::Map<String, Value> =
                fields.iter().map(|(k, v)| ((*k).to_owned(), Value::String((*v).to_owned()))).collect();
            ("application/json", Value::Object(map).to_string())
        }
    }
}

/// The OAuth error code of a failed response, or `HTTP <status>`: read from `error` (a
/// string, or an object's `code` or `type`), then `error_code` (research R10,
/// `tokenRefresh/providers.js:244`).
fn error_code(status: u16, body: &[u8]) -> String {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let code = match &v["error"] {
        Value::String(s) => Some(s.as_str()),
        Value::Object(o) => o.get("code").or_else(|| o.get("type")).and_then(Value::as_str),
        _ => None,
    }
    .filter(|c| !c.is_empty())
    .or_else(|| v["error_code"].as_str());
    // An error code is a short public token; cap it so an odd provider can't echo much.
    code.filter(|c| !c.is_empty()).map_or_else(|| format!("HTTP {status}"), |c| c.chars().take(80).collect())
}

/// POSTs an encoded token request to `token_url` and reads the grant.
pub async fn token_request(
    http: &SignInHttp,
    token_url: &str,
    body: (&'static str, String),
) -> Result<TokenGrant, SignInError> {
    const WHAT: &str = "token endpoint";
    let (status, bytes) = http.send(WHAT, reqwest::Method::POST, token_url, HeaderMap::new(), Some(body)).await?;
    if !(200..300).contains(&status) {
        return Err(SignInError::Rejected { what: WHAT, status, code: error_code(status, &bytes) });
    }
    let bad = |reason: &str| SignInError::BadResponse { what: WHAT, reason: reason.into() };
    let mut v: Value = serde_json::from_slice(&bytes).map_err(|_| bad("not JSON"))?;
    let mut take = |k: &str| match v.get_mut(k).map(Value::take) {
        Some(Value::String(s)) if !s.is_empty() => Some(s),
        _ => None,
    };
    let access_token = take("access_token").map(SecretString::new).ok_or_else(|| bad("no access_token"))?;
    let refresh_token = take("refresh_token").map(SecretString::new);
    let id_token = take("id_token").map(SecretString::new);
    let scope = take("scope");
    let expires_in = match &v["expires_in"] {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|s| s.is_finite() && *s > 0.0)
    .map(Duration::from_secs_f64);
    Ok(TokenGrant { access_token, refresh_token, expires_in, scope, id_token })
}

/// The payload of a JWT, unverified: for display claims only, never for trust.
pub fn jwt_payload(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let (_, payload, _) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok().filter(Value::is_object)
}

/// `path`'s first resolving alternative in `root`, as display text.
fn lookup(root: &Value, path: &ValuePath) -> Option<String> {
    path.alternatives().find_map(|alt| {
        let v = if alt == "." { root } else { alt.split('.').try_fold(root, |v, k| v.get(k))? };
        match v {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    })
}

/// Claims from the profile response and the id token (as `id_token.<claim>`). Without a
/// declared `email` path, the id token's `email` or `preferred_username`.
fn read_claims(decl: Option<&SignInProfile>, profile: Option<Value>, id_token: Option<Value>) -> Claims {
    let mut root = profile.filter(Value::is_object).unwrap_or_else(|| Value::Object(Default::default()));
    if let (Some(o), Some(id)) = (root.as_object_mut(), id_token) {
        o.entry("id_token").or_insert(id);
    }
    let default_email = ValuePath(vec!["id_token.email".into(), "id_token.preferred_username".into()]);
    let path = |f: fn(&SignInProfile) -> Option<&ValuePath>| decl.and_then(f);
    Claims {
        email: lookup(&root, path(|p| p.email.as_ref()).unwrap_or(&default_email)),
        user_id: path(|p| p.user_id.as_ref()).and_then(|p| lookup(&root, p)),
        tier: path(|p| p.tier.as_ref()).and_then(|p| lookup(&root, p)),
    }
}

fn auth_value(auth: &EndpointAuth, token: &SecretString) -> Option<HeaderValue> {
    let raw = token.with_exposed(|t| match auth.scheme {
        AuthScheme::Bearer => Some(format!("Bearer {t}")),
        AuthScheme::Raw => Some(t.to_owned()),
        _ => None,
    })?;
    let mut v = HeaderValue::from_str(&raw).ok()?;
    v.set_sensitive(true);
    Some(v)
}

/// The optional `[signin.profile]` read. Best effort: a failure leaves the claims to the
/// id token.
async fn fetch_profile(
    http: &SignInHttp,
    p: &SignInProfile,
    auth: &EndpointAuth,
    token: &SecretString,
) -> Option<Value> {
    let mut headers = HeaderMap::new();
    for (k, v) in &p.headers {
        headers.insert(HeaderName::from_bytes(k.as_bytes()).ok()?, HeaderValue::from_str(v).ok()?);
    }
    headers.insert(HeaderName::from_bytes(auth.header.as_bytes()).ok()?, auth_value(auth, token)?);
    match http.send("profile", reqwest::Method::GET, &p.url, headers, None).await {
        Ok((status, body)) if (200..300).contains(&status) => serde_json::from_slice(&body).ok(),
        Ok((status, _)) => {
            tracing::debug!(status, "sign-in profile read refused; claims from the id token only");
            None
        }
        Err(e) => {
            tracing::debug!(error = %e, "sign-in profile read failed; claims from the id token only");
            None
        }
    }
}

/// Whether `candidate` (from the discovery document) stays on the plugin's declared
/// sign-in hosts: same host and port as one of the declared sign-in URLs, and https or the
/// discovery URL's own scheme.
fn on_declared_hosts(decl: &SignInDecl, discovery: &Url, candidate: &Value) -> Option<String> {
    let raw = candidate.as_str()?.trim();
    let u = Url::parse(raw).ok()?;
    if u.scheme() != "https" && u.scheme() != discovery.scheme() {
        return None;
    }
    let authority = |u: &Url| Some((u.host_str()?.to_ascii_lowercase(), u.port_or_known_default()?));
    let want = authority(&u)?;
    let declared: BTreeSet<_> = decl.flow_urls().filter_map(|(_, d)| authority(&Url::parse(d).ok()?)).collect();
    declared.contains(&want).then(|| raw.to_owned())
}

/// The authorize and token URLs: discovery's when both stay on the declared hosts, else the
/// declared ones (`ref/9router/src/lib/oauth/services/xai.js:26-47`).
async fn endpoints(http: &SignInHttp, decl: &SignInDecl) -> (Option<String>, String) {
    let declared = (decl.authorize_url.clone(), decl.token_url.clone());
    let Some(discovery) = decl.discovery_url.as_deref() else { return declared };
    let Ok(base) = http.url(discovery) else { return declared };
    let doc = match http.send("discovery", reqwest::Method::GET, discovery, HeaderMap::new(), None).await {
        Ok((status, body)) if (200..300).contains(&status) => serde_json::from_slice::<Value>(&body).ok(),
        _ => None,
    };
    let Some(doc) = doc else {
        tracing::debug!("sign-in discovery unavailable; using the declared URLs");
        return declared;
    };
    let token = on_declared_hosts(decl, &base, &doc["token_endpoint"]);
    let authorize = match decl.flow {
        SignInFlow::Pkce => on_declared_hosts(decl, &base, &doc["authorization_endpoint"]).map(Some),
        SignInFlow::DeviceCode => Some(None),
    };
    match (authorize, token) {
        (Some(a), Some(t)) => (a, t),
        _ => {
            tracing::warn!("sign-in discovery points off the declared hosts; using the declared URLs");
            declared
        }
    }
}

/// What every flow carries: who is signing in and where the tokens go.
#[derive(Debug, Clone)]
pub(crate) struct Flow {
    provider: String,
    decl: SignInDecl,
    hosts: BTreeSet<String>,
    token_url: String,
}

impl Flow {
    /// The grant as a token entry for `name`, with claims from the id token and profile.
    async fn finish(&self, http: &SignInHttp, name: &str, grant: TokenGrant) -> TokenEntry {
        let id = grant.id_token.as_ref().and_then(|t| t.with_exposed(jwt_payload));
        let profile = match &self.decl.profile {
            Some(p) => fetch_profile(http, p, &self.decl.auth, &grant.access_token).await,
            None => None,
        };
        let claims = read_claims(self.decl.profile.as_ref(), profile, id);
        let now = SystemTime::now();
        TokenEntry {
            provider: self.provider.clone(),
            name: name.to_owned(),
            access_token: grant.access_token,
            refresh_token: grant.refresh_token,
            expires_at: now + grant.expires_in.unwrap_or(DEFAULT_EXPIRES_IN),
            scope: grant.scope.unwrap_or_else(|| self.decl.scopes.join(" ")),
            claims,
            hosts: self.hosts.clone(),
            signed_in_at: now,
            last_refresh_at: None,
            state: None,
            state_since: None,
            state_reason: None,
        }
    }
}

/// A started sign-in.
#[derive(Debug)]
pub enum Begun {
    Pkce(PkceSignIn),
    Device(DeviceSignIn),
}

/// Starts `provider`'s sign-in: discovery, then the device request or the PKCE setup
/// (verifier, state, loopback listener, authorize URL).
pub async fn begin(http: &SignInHttp, provider: &ProviderEntity) -> Result<Begun, SignInError> {
    let decl = provider.signin.as_ref().ok_or_else(|| SignInError::NotSignIn(provider.id.clone()))?;
    let (authorize, token_url) = endpoints(http, decl).await;
    let flow = Flow { provider: provider.id.clone(), decl: decl.clone(), hosts: provider.token_hosts(), token_url };
    match decl.flow {
        SignInFlow::DeviceCode => DeviceSignIn::start(http, flow).await.map(Begun::Device),
        SignInFlow::Pkce => {
            let base = authorize.ok_or_else(|| SignInError::BadResponse {
                what: "discovery",
                reason: "no authorization endpoint and no declared authorize_url".into(),
            })?;
            Ok(Begun::Pkce(PkceSignIn::start(flow, &base).await))
        }
    }
}

/// An authorization-code sign-in with PKCE, waiting for its code.
#[derive(Debug)]
pub struct PkceSignIn {
    flow: Flow,
    authorize_url: String,
    redirect_uri: String,
    kind: RedirectKind,
    verifier: SecretString,
    state: String,
    listener: Option<Loopback>,
    port_busy: Option<String>,
    started: Instant,
    timeout: Duration,
}

enum Got {
    Callback(std::io::Result<String>),
    Paste(Option<String>),
}

async fn accept_on(l: Option<&Loopback>, state: &str) -> std::io::Result<String> {
    match l {
        Some(l) => l.accept(|q| pkce::from_query(q, state).is_ok()).await,
        None => std::future::pending().await,
    }
}

impl PkceSignIn {
    /// Picks the redirect: declared order, a code page at once, a loopback redirect when its
    /// address binds. When none binds, the first declared one is used without a listener.
    async fn start(flow: Flow, authorize_base: &str) -> Self {
        let mut chosen = None;
        let mut listener = None;
        let mut port_busy = None;
        for r in &flow.decl.redirect {
            match r.kind {
                RedirectKind::CodePage => {
                    chosen = Some((r.uri.clone(), r.kind));
                    break;
                }
                RedirectKind::Loopback => match Loopback::bind(&r.uri).await {
                    Ok(l) => {
                        chosen = Some((l.redirect_uri().to_owned(), r.kind));
                        listener = Some(l);
                        port_busy = None;
                        break;
                    }
                    Err(e) => {
                        port_busy.get_or_insert(e.to_string());
                    }
                },
            }
        }
        let (redirect_uri, kind) = chosen
            .or_else(|| flow.decl.redirect.first().map(|r| (r.uri.clone(), r.kind)))
            .unwrap_or_else(|| (String::new(), RedirectKind::Loopback));
        let verifier = pkce::verifier(flow.decl.verifier_len());
        let state = pkce::state();
        let authorize_url =
            pkce::authorize_url(authorize_base, &flow.decl, &redirect_uri, &pkce::challenge(&verifier), &state);
        Self {
            flow,
            authorize_url,
            redirect_uri,
            kind,
            verifier,
            state,
            listener,
            port_busy,
            started: Instant::now(),
            timeout: BROWSER_TIMEOUT,
        }
    }

    /// Replaces the [`BROWSER_TIMEOUT`], counted from [`begin`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The link the operator opens. Not secret (it carries only the challenge and state).
    pub fn authorize_url(&self) -> &str {
        &self.authorize_url
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn redirect_kind(&self) -> RedirectKind {
        self.kind
    }

    /// Whether the loopback listener is waiting for the redirect.
    pub fn listening(&self) -> bool {
        self.listener.is_some()
    }

    /// Why no listener could be bound (port in use), for the operator.
    pub fn port_busy(&self) -> Option<&str> {
        self.port_busy.as_deref()
    }

    /// Whether the provider wants the terms warning shown (research R4).
    pub fn terms_warning(&self) -> bool {
        self.flow.decl.terms_warning
    }

    /// Reads an operator paste: the full redirect address, or for a code page `code#state`
    /// or a bare code.
    pub fn parse_paste(&self, input: &str) -> Result<SecretString, PasteError> {
        pkce::parse_paste(input, self.kind, &self.state)
    }

    /// Waits for the loopback redirect or `paste` (one operator line; `None` when input
    /// ended), whichever comes first, until the timeout or `cancel`. A refused paste returns
    /// its error and leaves the listener running: call again with the next line.
    pub async fn wait_code(
        &mut self,
        paste: impl Future<Output = Option<String>>,
        cancel: &CancellationToken,
    ) -> Result<SecretString, SignInError> {
        let deadline = tokio::time::sleep(self.timeout.saturating_sub(self.started.elapsed()));
        tokio::pin!(paste, deadline);
        let mut paste_open = true;
        loop {
            if !paste_open && self.listener.is_none() {
                return Err(SignInError::NoInput);
            }
            let got = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(SignInError::Cancelled),
                () = &mut deadline => return Err(SignInError::TimedOut(self.timeout)),
                q = accept_on(self.listener.as_ref(), &self.state) => Got::Callback(q),
                line = &mut paste, if paste_open => Got::Paste(line),
            };
            match got {
                Got::Callback(Ok(query)) => return pkce::from_query(&query, &self.state).map_err(Into::into),
                Got::Callback(Err(e)) => {
                    tracing::warn!(error = %e, "sign-in loopback listener failed; waiting for a paste");
                    self.listener = None;
                }
                Got::Paste(Some(line)) => return self.parse_paste(&line).map_err(Into::into),
                Got::Paste(None) => paste_open = false,
            }
        }
    }

    /// Exchanges `code` for tokens (anthropic's JSON body includes `state`,
    /// `ref/9router/src/lib/oauth/providers/claude.js:29-43`), reads the claims, and returns
    /// the entry for account `name`.
    pub async fn complete(&self, http: &SignInHttp, name: &str, code: SecretString) -> Result<TokenEntry, SignInError> {
        let decl = &self.flow.decl;
        let body = code.with_exposed(|c| {
            self.verifier.with_exposed(|v| {
                let mut fields = vec![("code", c)];
                if decl.body == TokenBody::Json {
                    fields.push(("state", self.state.as_str()));
                }
                fields.extend([
                    ("grant_type", "authorization_code"),
                    ("client_id", decl.client_id.as_str()),
                    ("redirect_uri", self.redirect_uri.as_str()),
                    ("code_verifier", v),
                ]);
                encode_body(decl.body, &fields)
            })
        });
        let grant = token_request(http, &self.flow.token_url, body).await?;
        Ok(self.flow.finish(http, name, grant).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jwt_payload_is_read_without_verification() {
        let enc = |v: &Value| URL_SAFE_NO_PAD.encode(v.to_string());
        let t = format!("{}.{}.x", enc(&json!({ "alg": "RS256" })), enc(&json!({ "email": "a@b.c" })));
        assert_eq!(jwt_payload(&t).unwrap()["email"], "a@b.c");
        assert_eq!(jwt_payload(&format!("{t}=")).unwrap()["email"], "a@b.c", "padding tolerated");
        assert!(jwt_payload("a.b").is_none() && jwt_payload("a.b.c.d").is_none() && jwt_payload("x.%%.y").is_none());
    }

    #[test]
    fn claims_prefer_the_declared_paths() {
        let p: SignInProfile = toml::from_str(
            "url = \"https://x.example/u\"\nemail = \"email | id_token.email\"\nuser_id = \"userId | principalId\"\ntier = \"sub.tier\"",
        )
        .unwrap();
        let id = Some(json!({ "email": "id@x", "preferred_username": "pu" }));
        let c = read_claims(Some(&p), Some(json!({ "principalId": 7, "sub": { "tier": "Pro" } })), id.clone());
        assert_eq!(c, Claims { email: Some("id@x".into()), user_id: Some("7".into()), tier: Some("Pro".into()) });
        let c = read_claims(None, None, Some(json!({ "preferred_username": "pu" })));
        assert_eq!(c.email.as_deref(), Some("pu"));
        assert_eq!(read_claims(None, None, None), Claims::default());
    }

    #[test]
    fn bodies_and_error_codes() {
        let (ct, b) = encode_body(TokenBody::Form, &[("a", "x y"), ("b", "1&2")]);
        assert_eq!((ct, b.as_str()), ("application/x-www-form-urlencoded", "a=x+y&b=1%262"));
        let (ct, b) = encode_body(TokenBody::Json, &[("code", "c"), ("state", "s")]);
        assert_eq!((ct, b.as_str()), ("application/json", r#"{"code":"c","state":"s"}"#));
        assert_eq!(error_code(400, br#"{"error":"invalid_grant"}"#), "invalid_grant");
        assert_eq!(error_code(403, br#"{"error":{"type":"permission_error"}}"#), "permission_error");
        assert_eq!(error_code(503, b"<html>"), "HTTP 503");
        assert_eq!(error_code(400, br#"{"error_code":"invalid_grant"}"#), "invalid_grant");
    }
}
