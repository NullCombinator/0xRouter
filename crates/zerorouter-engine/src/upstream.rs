//! The upstream HTTP client (research R18, R22): one pooled client per process, redirects
//! never followed, every resolved address re-checked against the SSRF rules, and the only
//! place an account secret is written into a request.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Url};
use zerorouter_registry::SecretString;
use zerorouter_registry::floor::Floor;
use zerorouter_registry::schema::{AuthScheme, Endpoint, ForwardMerge, ProviderEntity};
use zerorouter_registry::validate::is_private_ip;

use crate::redact::Redactor;

pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;
pub const POOL_IDLE: Duration = Duration::from_secs(90);
pub const TCP_KEEPALIVE: Duration = Duration::from_secs(60);

/// 9router `envMs`: a positive integer from the environment (leading digits, like
/// `parseInt`), else `default`.
pub fn env_ms(name: &str, default: u64) -> u64 {
    let Ok(raw) = std::env::var(name) else { return default };
    let digits: String = raw.trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|n| *n > 0).unwrap_or(default)
}

/// Resolves through the system resolver, then refuses loopback, private, link-local and
/// metadata addresses unless the operator allowed private endpoints.
struct CheckedResolver {
    allow_private: bool,
}

impl Resolve for CheckedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow_private = self.allow_private;
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if !allow_private && let Some(bad) = addrs.iter().find(|a| is_private_ip(a.ip())) {
                return Err(format!("{host} resolves to {}, a private address; set allow_private_endpoints = true to allow it", bad.ip()).into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// The process-wide client. Build once; clone freely (the pool is shared).
pub fn client(allow_private: bool) -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .pool_idle_timeout(POOL_IDLE)
        .tcp_keepalive(TCP_KEEPALIVE)
        .connect_timeout(Duration::from_millis(env_ms("FETCH_CONNECT_TIMEOUT_MS", DEFAULT_TIMEOUT_MS)))
        .dns_resolver(Arc::new(CheckedResolver { allow_private }))
        .build()
        .expect("the TLS backend initialises")
}

/// A request ready to send. `header_timeout` bounds the wait for response headers.
#[derive(Debug)]
pub struct Outgoing {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub header_timeout: Duration,
}

impl Outgoing {
    pub fn into_request(self, client: &reqwest::Client) -> reqwest::RequestBuilder {
        client.request(self.method, self.url).headers(self.headers).body(self.body)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error("{0:?} can't be a URL path segment")]
    BadSegment(String),
    #[error("endpoint URL {url}: {reason}")]
    BadUrl { url: String, reason: String },
    #[error("provider {0} has no key for this account")]
    NoSecret(String),
    #[error("auth scheme {0:?} is not supported")]
    UnsupportedAuth(String),
    #[error("header {0}: invalid value")]
    BadHeader(String),
}

/// What goes into one upstream request.
#[derive(Clone)]
pub struct RequestParts<'a> {
    pub provider: &'a ProviderEntity,
    pub endpoint: &'a Endpoint,
    pub floor: &'a Floor,
    pub redactor: &'a Redactor,
    /// `None` for a provider with `auth.no_auth`.
    pub secret: Option<&'a SecretString>,
    pub client_style: &'a str,
    pub client_headers: &'a HeaderMap,
    pub model: &'a str,
    pub voice: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub body: Bytes,
}

/// `s` as one path segment: everything but unreserved characters percent-encoded.
fn segment(s: &str) -> Result<String, BuildError> {
    if s.is_empty() || s == "." || s == ".." {
        return Err(BuildError::BadSegment(s.to_owned()));
    }
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(out)
}

pub fn endpoint_url(template: &str, model: &str, voice: Option<&str>) -> Result<Url, BuildError> {
    let mut url = template.replace("{model}", &segment(model)?);
    if url.contains("{voice}") {
        url = url.replace("{voice}", &segment(voice.unwrap_or_default())?);
    }
    Url::parse(&url).map_err(|e| BuildError::BadUrl { url: template.to_owned(), reason: e.to_string() })
}

/// The header the core writes the secret into, and its value.
fn auth_header(p: &ProviderEntity, secret: &SecretString) -> Result<(HeaderName, HeaderValue), BuildError> {
    let auth = p.auth.as_ref();
    let name = auth.and_then(|a| a.header.as_deref()).map_or(AUTHORIZATION, |h| {
        HeaderName::from_bytes(h.as_bytes()).unwrap_or(AUTHORIZATION)
    });
    let scheme = auth.and_then(|a| a.scheme).unwrap_or(if name == AUTHORIZATION { AuthScheme::Bearer } else { AuthScheme::Raw });
    let mut value = secret.with_exposed(|s| match scheme {
        AuthScheme::Bearer => Ok(HeaderValue::from_str(&format!("Bearer {s}"))),
        AuthScheme::Raw => Ok(HeaderValue::from_str(s)),
        other => Err(BuildError::UnsupportedAuth(other.to_string())),
    })?
    .map_err(|_| BuildError::BadHeader(name.to_string()))?;
    value.set_sensitive(true);
    Ok((name, value))
}

fn name_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix),
        None => pattern.eq_ignore_ascii_case(name),
    }
}

/// Client headers the provider's forwarding rules pass, minus the floor, CR/LF values and
/// values that contain a secret.
fn forwarded(parts: &RequestParts) -> Vec<(HeaderName, HeaderValue, ForwardMerge)> {
    let Some(fwd) = parts.provider.forwarding.as_ref() else { return Vec::new() };
    let mut out = Vec::new();
    for rule in &fwd.to_upstream.headers {
        if !rule.from_styles.is_empty() && !rule.from_styles.iter().any(|s| s == parts.client_style) {
            continue;
        }
        for (name, value) in parts.client_headers {
            if !name_matches(&rule.name, name.as_str()) || parts.floor.blocks(name.as_str()) {
                continue;
            }
            let bytes = value.as_bytes();
            if bytes.iter().any(|b| matches!(b, b'\r' | b'\n')) {
                continue;
            }
            let Ok(text) = value.to_str() else { continue };
            if matches!(parts.redactor.redact(text), std::borrow::Cow::Owned(_)) {
                continue;
            }
            out.push((name.clone(), value.clone(), rule.merge));
        }
    }
    out
}

/// Assembles the request: client headers → floor → plugin static headers → core auth.
pub fn build_request(parts: RequestParts) -> Result<Outgoing, BuildError> {
    let e = parts.endpoint;
    let url = endpoint_url(&e.url, parts.model, parts.voice)?;
    let method = Method::from_bytes(e.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| BuildError::BadUrl { url: e.url.clone(), reason: format!("method {}", e.method) })?;

    let mut headers = HeaderMap::new();
    let mut append: Vec<(HeaderName, HeaderValue)> = Vec::new();
    for (name, value, merge) in forwarded(&parts) {
        match merge {
            ForwardMerge::AppendCsv => append.push((name.clone(), value.clone())),
            ForwardMerge::Replace => {}
        }
        headers.append(name, value);
    }
    for (k, v) in &e.headers {
        let name = HeaderName::from_bytes(k.as_bytes()).map_err(|_| BuildError::BadHeader(k.clone()))?;
        if parts.floor.blocks(name.as_str()) {
            continue;
        }
        let mut value = v.clone();
        for (_, extra) in append.iter().filter(|(n, _)| *n == name) {
            value = format!("{value},{}", extra.to_str().unwrap_or_default());
        }
        headers.insert(name, HeaderValue::from_str(&value).map_err(|_| BuildError::BadHeader(k.clone()))?);
    }
    if let Some(ct) = parts.content_type {
        headers.insert(reqwest::header::CONTENT_TYPE, HeaderValue::from_str(ct).map_err(|_| BuildError::BadHeader("content-type".into()))?);
    }
    let no_auth = parts.provider.auth.as_ref().is_some_and(|a| a.no_auth);
    if !no_auth {
        let secret = parts.secret.ok_or_else(|| BuildError::NoSecret(parts.provider.id.clone()))?;
        let (name, value) = auth_header(parts.provider, secret)?;
        headers.insert(name, value);
    }
    let header_timeout = Duration::from_millis(e.timeout_ms.unwrap_or_else(|| env_ms("FETCH_CONNECT_TIMEOUT_MS", DEFAULT_TIMEOUT_MS)));
    Ok(Outgoing { method, url, headers, body: parts.body, header_timeout })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerorouter_registry::schema::AuthDecl;

    fn provider(src: &str) -> ProviderEntity {
        zerorouter_registry::validate_user_plugin(src, std::path::Path::new("acme.toml")).unwrap_or_else(|e| panic!("{e:#?}"))
    }

    #[test]
    fn segments_are_encoded_and_dots_refused() {
        assert_eq!(segment("claude-3.5/x y").unwrap(), "claude-3.5%2Fx%20y");
        assert!(segment("..").is_err() && segment(".").is_err() && segment("").is_err());
        let u = endpoint_url("https://h.example/v1beta/models/{model}:generateContent", "gemini/2.5", None).unwrap();
        assert_eq!(u.as_str(), "https://h.example/v1beta/models/gemini%2F2.5:generateContent");
    }

    #[test]
    fn env_ms_follows_parse_int() {
        assert_eq!(env_ms("ZR_TEST_UNSET_VAR_X", 7), 7);
    }

    #[test]
    fn headers_follow_forwarding_floor_and_auth_last() {
        let mut p: ProviderEntity = provider(
            r#"
schema = 2
id = "acme"
category = "apikey"
[auth]
header = "x-api-key"
scheme = "raw"
[forwarding.to_upstream]
headers = [
  { name = "anthropic-beta", merge = "append_csv", from_styles = ["anthropic-messages"] },
  { name = "x-trace-*" },
]
[[endpoints.text]]
url = "https://api.acme.example/v1/messages"
wire = "anthropic-messages"
headers = { "anthropic-beta" = "base-1", "anthropic-version" = "2023-06-01" }
"#,
        );
        let secret = SecretString::new("sk-acme-SECRET-1");
        let redactor = Redactor::new([&secret]);
        let floor = Floor::default();
        let mut client = HeaderMap::new();
        client.insert("anthropic-beta", HeaderValue::from_static("client-2"));
        client.insert("x-trace-id", HeaderValue::from_static("t1"));
        client.insert("x-trace-leak", HeaderValue::from_static("has sk-acme-SECRET-1 inside"));
        client.insert("x-api-key", HeaderValue::from_static("0r-client-key"));
        client.insert("x-other", HeaderValue::from_static("nope"));
        let endpoint = p.endpoints.values().next().unwrap().0[0].clone();
        let base = RequestParts {
            provider: &p,
            endpoint: &endpoint,
            floor: &floor,
            redactor: &redactor,
            secret: Some(&secret),
            client_style: "anthropic-messages",
            client_headers: &client,
            model: "m",
            voice: None,
            content_type: Some("application/json"),
            body: Bytes::from_static(b"{}"),
        };
        let out = build_request(base.clone()).unwrap();
        let h = &out.headers;
        assert_eq!(h["anthropic-beta"], "base-1,client-2");
        assert_eq!(h["anthropic-version"], "2023-06-01");
        assert_eq!(h["x-trace-id"], "t1");
        assert!(h.get("x-trace-leak").is_none(), "a value holding a secret is dropped");
        assert!(h.get("x-other").is_none());
        assert_eq!(h["x-api-key"], "sk-acme-SECRET-1", "core auth is last and replaces the client's key");
        assert!(h["x-api-key"].is_sensitive());
        assert_eq!(out.header_timeout, Duration::from_millis(DEFAULT_TIMEOUT_MS));

        let other = build_request(RequestParts { client_style: "openai-chat", ..base }).unwrap();
        assert_eq!(other.headers["anthropic-beta"], "base-1", "from_styles limits the rule");

        p.auth = Some(AuthDecl { header: None, scheme: None, ..AuthDecl::default() });
        let base = RequestParts { provider: &p, client_style: "openai-chat", ..other_parts(&endpoint, &floor, &redactor, &secret, &client) };
        let bearer = build_request(base.clone()).unwrap();
        assert_eq!(bearer.headers[AUTHORIZATION], "Bearer sk-acme-SECRET-1");
        let none = RequestParts { secret: None, ..base };
        assert_eq!(build_request(none).unwrap_err(), BuildError::NoSecret("acme".into()));
    }

    fn other_parts<'a>(
        endpoint: &'a Endpoint,
        floor: &'a Floor,
        redactor: &'a Redactor,
        secret: &'a SecretString,
        client: &'a HeaderMap,
    ) -> RequestParts<'a> {
        static EMPTY: std::sync::LazyLock<ProviderEntity> = std::sync::LazyLock::new(|| {
            provider("schema = 2\nid = \"empty\"\ncategory = \"apikey\"\n[[endpoints.text]]\nurl = \"https://e.example/v1\"\nwire = \"openai-chat\"\n")
        });
        RequestParts {
            provider: &EMPTY,
            endpoint,
            floor,
            redactor,
            secret: Some(secret),
            client_style: "",
            client_headers: client,
            model: "m",
            voice: None,
            content_type: None,
            body: Bytes::new(),
        }
    }

    #[tokio::test]
    async fn resolver_refuses_private_addresses() {
        let r = CheckedResolver { allow_private: false };
        let name: Name = "localhost".parse().unwrap();
        assert!(r.resolve(name).await.is_err());
        let r = CheckedResolver { allow_private: true };
        assert!(r.resolve("localhost".parse().unwrap()).await.is_ok());
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf).await;
            s.write_all(b"HTTP/1.1 302 Found\r\nlocation: http://169.254.169.254/\r\ncontent-length: 0\r\n\r\n").await.unwrap();
        });
        let resp = client(true).get(format!("http://{addr}/")).send().await.unwrap();
        assert_eq!(resp.status(), 302);
    }
}
