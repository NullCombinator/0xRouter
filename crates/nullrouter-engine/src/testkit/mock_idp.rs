//! A scripted identity provider on `127.0.0.1:0` for sign-in and refresh tests.
//!
//! Serves the device grant (RFC 8628), the PKCE authorize redirect and code exchange (the
//! verifier is checked with S256, and `state` when the body carries it), the OpenID
//! discovery document, the refresh grant with optional rotation, and a profile endpoint.
//! Token-endpoint failures are scripted per test; access tokens live as long as the test
//! sets (down to 2 s or less) and are checked by [`MockIdp::token_valid`] and the
//! `/v1/user` and `/v1/check` routes.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::Response;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;

pub const DEVICE: &str = "/oauth2/device";
pub const TOKEN: &str = "/oauth2/token";
pub const AUTHORIZE: &str = "/oauth2/authorize";
pub const DISCOVERY: &str = "/.well-known/openid-configuration";
/// The profile: the bearer token's user, or 401.
pub const PROFILE: &str = "/v1/user";
/// 200 for a live bearer token, else 401.
pub const CHECK: &str = "/v1/check";

/// The client id the mock accepts unless changed.
pub const CLIENT_ID: &str = "mock-client";
pub const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// One scripted answer to a device-grant poll; with none queued the grant is approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    SlowDown,
    Expired,
    Denied,
}

impl DevicePoll {
    fn code(self) -> &'static str {
        match self {
            Self::Pending => "authorization_pending",
            Self::SlowDown => "slow_down",
            Self::Expired => "expired_token",
            Self::Denied => "access_denied",
        }
    }
}

/// One scripted token-endpoint failure, taken by the next token request of any grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// `{ "error": code }` with `status`.
    Error { status: u16, code: String },
    /// A bare status with a short JSON body.
    Status(u16),
    /// No response for this long, then a 504 (a client timeout shorter than this sees a
    /// timeout).
    Hang(Duration),
}

impl Failure {
    /// A 400 with an OAuth error code, e.g. `invalid_grant` (research R10's permanent set).
    pub fn permanent(code: &str) -> Self {
        Self::Error { status: 400, code: code.into() }
    }
}

/// A token request as received. Holds codes and verifiers: test data only.
#[derive(Debug, Clone)]
pub struct TokenRequest {
    pub json: bool,
    pub path_and_query: String,
    pub fields: BTreeMap<String, String>,
}

impl TokenRequest {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.fields.get(k).map(String::as_str)
    }
}

#[derive(Debug)]
struct CodeGrant {
    challenge: String,
    state: String,
    redirect_uri: String,
}

struct Idp {
    client_id: Mutex<String>,
    lifetime: Mutex<Duration>,
    rotate: AtomicBool,
    issue_refresh: AtomicBool,
    deny_authorize: AtomicBool,
    device_interval: AtomicU64,
    device_expires_in: AtomicU64,
    device_script: Mutex<VecDeque<DevicePoll>>,
    device_codes: Mutex<Vec<String>>,
    failures: Mutex<VecDeque<Failure>>,
    discovery: Mutex<Option<Value>>,
    profile: Mutex<Value>,
    id_claims: Mutex<Option<Value>>,
    codes: Mutex<HashMap<String, CodeGrant>>,
    access: Mutex<HashMap<String, Instant>>,
    /// Refresh token → still usable.
    refresh: Mutex<HashMap<String, bool>>,
    token_requests: Mutex<Vec<TokenRequest>>,
    authorize_queries: Mutex<Vec<String>>,
    device_requests: Mutex<Vec<TokenRequest>>,
    token_calls: AtomicUsize,
    refresh_calls: AtomicUsize,
    device_calls: AtomicUsize,
    profile_calls: AtomicUsize,
    seq: AtomicUsize,
    base: Mutex<String>,
}

pub struct MockIdp {
    addr: SocketAddr,
    inner: Arc<Idp>,
    task: JoinHandle<()>,
}

impl Drop for MockIdp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `fields` of a JSON object or form body.
pub fn body_fields(json: bool, body: &[u8]) -> BTreeMap<String, String> {
    if json {
        let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        v.as_object()
            .map(|o| {
                o.iter().map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_owned))).collect()
            })
            .unwrap_or_default()
    } else {
        url::form_urlencoded::parse(body).into_owned().collect()
    }
}

/// An unsigned JWT (`alg: none`) carrying `claims`.
pub fn unsigned_jwt(claims: &Value) -> String {
    let enc = |v: &Value| URL_SAFE_NO_PAD.encode(v.to_string());
    format!("{}.{}.sig", enc(&json!({ "alg": "none", "typ": "JWT" })), enc(claims))
}

impl MockIdp {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let inner = Arc::new(Idp {
            client_id: Mutex::new(CLIENT_ID.into()),
            lifetime: Mutex::new(Duration::from_secs(3600)),
            rotate: AtomicBool::new(true),
            issue_refresh: AtomicBool::new(true),
            deny_authorize: AtomicBool::new(false),
            device_interval: AtomicU64::new(1),
            device_expires_in: AtomicU64::new(600),
            device_script: Mutex::default(),
            device_codes: Mutex::default(),
            failures: Mutex::default(),
            discovery: Mutex::default(),
            profile: Mutex::new(json!({
                "email": "user@example.com",
                "userId": "user-0001",
                "subscriptionTier": "SuperGrok",
            })),
            id_claims: Mutex::default(),
            codes: Mutex::default(),
            access: Mutex::default(),
            refresh: Mutex::default(),
            token_requests: Mutex::default(),
            authorize_queries: Mutex::default(),
            device_requests: Mutex::default(),
            token_calls: AtomicUsize::new(0),
            refresh_calls: AtomicUsize::new(0),
            device_calls: AtomicUsize::new(0),
            profile_calls: AtomicUsize::new(0),
            seq: AtomicUsize::new(0),
            base: Mutex::new(format!("http://{addr}")),
        });
        let app = axum::Router::new().fallback(handle).with_state(inner.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, inner, task }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub fn set_client_id(&self, id: &str) {
        *lock(&self.inner.client_id) = id.into();
    }

    /// How long issued access tokens live (`expires_in` is this, rounded up, at least 1).
    pub fn set_token_lifetime(&self, d: Duration) {
        *lock(&self.inner.lifetime) = d;
    }

    /// Whether a refresh issues a new refresh token (and revokes the old one). Default on.
    pub fn set_rotation(&self, on: bool) {
        self.inner.rotate.store(on, Ordering::SeqCst);
    }

    /// Whether code and device grants issue a refresh token at all. Default on.
    pub fn set_issue_refresh(&self, on: bool) {
        self.inner.issue_refresh.store(on, Ordering::SeqCst);
    }

    /// The authorize endpoint redirects with `error=access_denied`.
    pub fn set_deny_authorize(&self, on: bool) {
        self.inner.deny_authorize.store(on, Ordering::SeqCst);
    }

    /// The device response's `interval` (seconds) and `expires_in`.
    pub fn set_device_timing(&self, interval: u64, expires_in: u64) {
        self.inner.device_interval.store(interval, Ordering::SeqCst);
        self.inner.device_expires_in.store(expires_in, Ordering::SeqCst);
    }

    /// Queues answers for the next device-grant polls; approved once the queue is empty.
    pub fn push_device(&self, polls: impl IntoIterator<Item = DevicePoll>) {
        lock(&self.inner.device_script).extend(polls);
    }

    /// Queues failures for the next token requests (any grant).
    pub fn fail_token(&self, failures: impl IntoIterator<Item = Failure>) {
        lock(&self.inner.failures).extend(failures);
    }

    /// Replaces the discovery document; `None` restores the default (this mock's URLs).
    pub fn set_discovery(&self, doc: Option<Value>) {
        *lock(&self.inner.discovery) = doc;
    }

    /// The JSON `/v1/user` returns.
    pub fn set_profile(&self, v: Value) {
        *lock(&self.inner.profile) = v;
    }

    /// Token responses carry an unsigned `id_token` with these claims.
    pub fn set_id_claims(&self, claims: Option<Value>) {
        *lock(&self.inner.id_claims) = claims;
    }

    /// Whether `token` was issued here and hasn't expired.
    pub fn token_valid(&self, token: &str) -> bool {
        lock(&self.inner.access).get(token).is_some_and(|exp| Instant::now() < *exp)
    }

    /// Every access token issued, newest last.
    pub fn issued(&self) -> Vec<String> {
        let mut v: Vec<(String, Instant)> = lock(&self.inner.access).iter().map(|(k, v)| (k.clone(), *v)).collect();
        v.sort_by_key(|(k, _)| k.rsplit('-').next().and_then(|n| n.parse::<usize>().ok()));
        v.into_iter().map(|(k, _)| k).collect()
    }

    pub fn token_requests(&self) -> Vec<TokenRequest> {
        lock(&self.inner.token_requests).clone()
    }

    pub fn device_requests(&self) -> Vec<TokenRequest> {
        lock(&self.inner.device_requests).clone()
    }

    /// Raw query strings the authorize endpoint received.
    pub fn authorize_queries(&self) -> Vec<String> {
        lock(&self.inner.authorize_queries).clone()
    }

    pub fn token_calls(&self) -> usize {
        self.inner.token_calls.load(Ordering::SeqCst)
    }

    pub fn refresh_calls(&self) -> usize {
        self.inner.refresh_calls.load(Ordering::SeqCst)
    }

    pub fn device_calls(&self) -> usize {
        self.inner.device_calls.load(Ordering::SeqCst)
    }

    pub fn profile_calls(&self) -> usize {
        self.inner.profile_calls.load(Ordering::SeqCst)
    }

    /// Issues an access token (current lifetime) and a refresh token at once, as a
    /// completed sign-in would: `(access, refresh, expires_in)`.
    pub fn grant(&self) -> (String, String, Duration) {
        let v = mint(&self.inner, true);
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
        (s("access_token"), s("refresh_token"), Duration::from_secs(v["expires_in"].as_u64().unwrap_or(1)))
    }

    /// Plays the browser: opens `authorize_url` (without following the redirect) and
    /// returns where the provider sends the operator, `redirect_uri?code=…&state=…`.
    pub async fn browse(&self, authorize_url: &str) -> String {
        let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().expect("client");
        let r = c.get(authorize_url).send().await.expect("authorize request");
        assert_eq!(r.status(), 302, "authorize: {}", r.text().await.unwrap_or_default());
        r.headers()[header::LOCATION].to_str().expect("location").to_owned()
    }
}

fn reply(status: u16, body: Value) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("response")
}

fn oauth_error(code: &str) -> Response {
    reply(400, json!({ "error": code, "error_description": format!("mock: {code}") }))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")
}

fn s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

async fn handle(State(idp): State<Arc<Idp>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body: Bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap_or_default();
    let path = parts.uri.path().to_owned();
    let pq = parts.uri.path_and_query().map_or_else(|| path.clone(), |p| p.to_string());
    let json_body = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    match (&parts.method, path.as_str()) {
        (&Method::GET, DISCOVERY) => {
            let base = lock(&idp.base).clone();
            let doc = lock(&idp.discovery).clone().unwrap_or_else(|| {
                json!({
                    "issuer": base,
                    "authorization_endpoint": format!("{base}{AUTHORIZE}"),
                    "token_endpoint": format!("{base}{TOKEN}"),
                    "device_authorization_endpoint": format!("{base}{DEVICE}"),
                })
            });
            reply(200, doc)
        }
        (&Method::GET, AUTHORIZE) => authorize(&idp, parts.uri.query().unwrap_or_default()),
        (&Method::POST, DEVICE) => {
            idp.device_calls.fetch_add(1, Ordering::SeqCst);
            let fields = body_fields(json_body, &body);
            lock(&idp.device_requests).push(TokenRequest {
                json: json_body,
                path_and_query: pq,
                fields: fields.clone(),
            });
            if fields.get("client_id") != Some(&lock(&idp.client_id)) {
                return reply(401, json!({ "error": "invalid_client" }));
            }
            let n = idp.seq.fetch_add(1, Ordering::SeqCst);
            let device_code = format!("mock-device-SENTINEL-{n}");
            lock(&idp.device_codes).push(device_code.clone());
            let base = lock(&idp.base).clone();
            reply(
                200,
                json!({
                    "device_code": device_code,
                    "user_code": format!("ABCD-{n:04}"),
                    "verification_uri": format!("{base}/device"),
                    "verification_uri_complete": format!("{base}/device?user_code=ABCD-{n:04}"),
                    "expires_in": idp.device_expires_in.load(Ordering::SeqCst),
                    "interval": idp.device_interval.load(Ordering::SeqCst),
                }),
            )
        }
        (&Method::POST, TOKEN) => token(&idp, json_body, &body, pq).await,
        (&Method::GET, PROFILE) | (&Method::GET, CHECK) => {
            if path == PROFILE {
                idp.profile_calls.fetch_add(1, Ordering::SeqCst);
            }
            let live = bearer(&parts.headers)
                .is_some_and(|t| lock(&idp.access).get(t).is_some_and(|exp| Instant::now() < *exp));
            match (live, path == PROFILE) {
                (false, _) => reply(401, json!({ "error": "invalid_token" })),
                (true, true) => reply(200, lock(&idp.profile).clone()),
                (true, false) => reply(200, json!({ "ok": true })),
            }
        }
        _ => reply(404, json!({ "error": format!("mock idp: nothing at {pq}") })),
    }
}

fn authorize(idp: &Idp, query: &str) -> Response {
    lock(&idp.authorize_queries).push(query.to_owned());
    let q: BTreeMap<String, String> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    let get = |k: &str| q.get(k).cloned().unwrap_or_default();
    let redirect_uri = get("redirect_uri");
    let bad = get("response_type") != "code"
        || q.get("client_id") != Some(&lock(&idp.client_id))
        || redirect_uri.is_empty()
        || get("code_challenge_method") != "S256"
        || get("code_challenge").is_empty()
        || get("state").is_empty();
    if bad {
        return reply(400, json!({ "error": "invalid_request" }));
    }
    let state = get("state");
    let mut to = url::Url::parse(&redirect_uri).expect("redirect uri parses");
    if idp.deny_authorize.load(Ordering::SeqCst) {
        to.query_pairs_mut().append_pair("error", "access_denied").append_pair("state", &state);
    } else {
        let code = format!("mock-code-SENTINEL-{}", idp.seq.fetch_add(1, Ordering::SeqCst));
        to.query_pairs_mut().append_pair("code", &code).append_pair("state", &state);
        lock(&idp.codes).insert(
            code,
            CodeGrant { challenge: get("code_challenge"), state: state.clone(), redirect_uri: redirect_uri.clone() },
        );
    }
    Response::builder().status(302).header(header::LOCATION, to.as_str()).body(Body::empty()).expect("response")
}

async fn token(idp: &Idp, json_body: bool, body: &[u8], path_and_query: String) -> Response {
    idp.token_calls.fetch_add(1, Ordering::SeqCst);
    let fields = body_fields(json_body, body);
    let grant = fields.get("grant_type").cloned().unwrap_or_default();
    if grant == "refresh_token" {
        idp.refresh_calls.fetch_add(1, Ordering::SeqCst);
    }
    lock(&idp.token_requests).push(TokenRequest { json: json_body, path_and_query, fields: fields.clone() });
    let failure = lock(&idp.failures).pop_front();
    match failure {
        Some(Failure::Error { status, code }) => return reply(status, json!({ "error": code })),
        Some(Failure::Status(s)) => return reply(s, json!({ "message": format!("mock: status {s}") })),
        Some(Failure::Hang(d)) => {
            tokio::time::sleep(d).await;
            return reply(504, json!({ "message": "mock: hung" }));
        }
        None => {}
    }
    if fields.get("client_id") != Some(&lock(&idp.client_id)) {
        return reply(401, json!({ "error": "invalid_client" }));
    }
    let get = |k: &str| fields.get(k).map(String::as_str).unwrap_or_default();
    let with_refresh = idp.issue_refresh.load(Ordering::SeqCst);
    match grant.as_str() {
        "authorization_code" => {
            let Some(g) = lock(&idp.codes).remove(get("code")) else { return oauth_error("invalid_grant") };
            if g.redirect_uri != get("redirect_uri") || g.challenge != s256(get("code_verifier")) {
                return oauth_error("invalid_grant");
            }
            if fields.get("state").is_some_and(|s| *s != g.state) {
                return oauth_error("invalid_request");
            }
            issue(idp, with_refresh)
        }
        DEVICE_GRANT => {
            if !lock(&idp.device_codes).iter().any(|c| c == get("device_code")) {
                return oauth_error("invalid_grant");
            }
            match lock(&idp.device_script).pop_front() {
                Some(p) => oauth_error(p.code()),
                None => {
                    lock(&idp.device_codes).retain(|c| c != get("device_code"));
                    issue(idp, with_refresh)
                }
            }
        }
        "refresh_token" => {
            let mut refresh = lock(&idp.refresh);
            match refresh.get(get("refresh_token")) {
                Some(true) => {}
                Some(false) => return oauth_error("refresh_token_reused"),
                None => return oauth_error("invalid_grant"),
            }
            let rotate = idp.rotate.load(Ordering::SeqCst);
            if rotate {
                refresh.insert(get("refresh_token").to_owned(), false);
            }
            drop(refresh);
            issue(idp, rotate)
        }
        _ => oauth_error("unsupported_grant_type"),
    }
}

fn issue(idp: &Idp, with_refresh: bool) -> Response {
    reply(200, mint(idp, with_refresh))
}

/// A token response body, its tokens registered as issued.
fn mint(idp: &Idp, with_refresh: bool) -> Value {
    let n = idp.seq.fetch_add(1, Ordering::SeqCst);
    let lifetime = *lock(&idp.lifetime);
    let access = format!("mock-access-SENTINEL-{n}");
    lock(&idp.access).insert(access.clone(), Instant::now() + lifetime);
    let mut body = json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": lifetime.as_secs_f64().ceil().max(1.0) as u64,
        "scope": "openid offline_access",
    });
    if with_refresh {
        let refresh = format!("mock-refresh-SENTINEL-{n}");
        lock(&idp.refresh).insert(refresh.clone(), true);
        body["refresh_token"] = json!(refresh);
    }
    if let Some(claims) = lock(&idp.id_claims).clone() {
        body["id_token"] = json!(unsigned_jwt(&claims));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_of(b: Vec<u8>) -> Value {
        serde_json::from_slice(&b).unwrap()
    }

    fn form(pairs: &[(&str, &str)]) -> String {
        url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish()
    }

    #[tokio::test]
    async fn code_exchange_checks_the_verifier_and_refresh_rotates() {
        let idp = MockIdp::start().await;
        let c = reqwest::Client::new();
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let auth = format!(
            "{}?response_type=code&client_id={CLIENT_ID}&redirect_uri=http%3A%2F%2F127.0.0.1%3A9%2Fcb&code_challenge={}&code_challenge_method=S256&state=st",
            idp.url(AUTHORIZE),
            s256(verifier)
        );
        let loc = idp.browse(&auth).await;
        let u = url::Url::parse(&loc).unwrap();
        let code = u.query_pairs().find(|(k, _)| k == "code").unwrap().1.into_owned();
        let post = |b: String| {
            c.post(idp.url(TOKEN)).header("content-type", "application/x-www-form-urlencoded").body(b).send()
        };
        let base =
            [("grant_type", "authorization_code"), ("client_id", CLIENT_ID), ("redirect_uri", "http://127.0.0.1:9/cb")];
        let mut bad = base.to_vec();
        bad.extend([("code", code.as_str()), ("code_verifier", "wrong")]);
        assert_eq!(post(form(&bad)).await.unwrap().status(), 400, "wrong verifier; the code is spent");

        let loc = idp.browse(&auth).await;
        let code = url::Url::parse(&loc).unwrap().query_pairs().find(|(k, _)| k == "code").unwrap().1.into_owned();
        let mut ok = base.to_vec();
        ok.extend([("code", code.as_str()), ("code_verifier", verifier)]);
        let v = json_of(post(form(&ok)).await.unwrap().bytes().await.unwrap().to_vec());
        let access = v["access_token"].as_str().unwrap();
        assert!(idp.token_valid(access));
        let refresh = v["refresh_token"].as_str().unwrap().to_owned();

        let r = |t: &str| form(&[("grant_type", "refresh_token"), ("client_id", CLIENT_ID), ("refresh_token", t)]);
        let v = json_of(post(r(&refresh)).await.unwrap().bytes().await.unwrap().to_vec());
        assert_ne!(v["refresh_token"].as_str().unwrap(), refresh, "rotated");
        let reused = json_of(post(r(&refresh)).await.unwrap().bytes().await.unwrap().to_vec());
        assert_eq!(reused["error"], "refresh_token_reused");
        assert_eq!(idp.refresh_calls(), 2);

        idp.fail_token([Failure::Status(503)]);
        assert_eq!(post(r("x")).await.unwrap().status(), 503);
        assert_eq!(idp.token_calls(), 5);
    }
}
