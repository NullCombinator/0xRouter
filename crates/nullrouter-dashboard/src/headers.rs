//! What every response carries, and the two refusals that come before any page (contracts/
//! dashboard-http.md "Every response"): a `Host` that isn't the dashboard's own is `421` (DNS
//! rebinding), and any method but `GET` is `405`, except `POST /signin`.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub const CSP: &str = "default-src 'none'; style-src 'self'; font-src 'self'; img-src 'self'; \
form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

/// Content-hashed assets: the one response that may be cached.
pub const IMMUTABLE: &str = "public, max-age=86400, immutable";

/// The `Host` values the dashboard answers to: its loopback names on the bound port, and the
/// configured host.
#[derive(Clone, Debug)]
pub struct HostRule {
    allowed: Arc<BTreeSet<String>>,
}

impl HostRule {
    /// `configured` is the host part of `[dashboard] listen` as written; `port` is the bound one.
    pub fn new(configured: &str, port: u16) -> Self {
        let bare = configured.trim_start_matches('[').trim_end_matches(']');
        let configured = if bare.contains(':') { format!("[{bare}]") } else { bare.to_owned() };
        let allowed = ["127.0.0.1", "localhost", "[::1]", configured.as_str()]
            .iter()
            .map(|h| format!("{}:{port}", h.to_ascii_lowercase()))
            .collect();
        Self { allowed: Arc::new(allowed) }
    }

    pub fn allows(&self, host: &str) -> bool {
        self.allowed.contains(&host.to_ascii_lowercase())
    }
}

/// Whether `method` may reach `path`.
pub fn method_allowed(method: &Method, path: &str) -> bool {
    *method == Method::GET || (*method == Method::POST && path == "/signin")
}

/// Sets the security headers on `headers`. `Cache-Control` is `no-store` unless the handler set
/// one (assets).
pub fn apply(headers: &mut HeaderMap) {
    let fixed: [(HeaderName, &'static str); 5] = [
        (header::CONTENT_SECURITY_POLICY, CSP),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (HeaderName::from_static("cross-origin-resource-policy"), "same-origin"),
    ];
    for (name, value) in fixed {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
}

fn plain(status: StatusCode, text: &'static str) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
}

/// The middleware around every route: Host (421), then method (405), then the handler, then the
/// headers on whatever came back.
pub async fn enforce(State(rule): State<HostRule>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned)
        .or_else(|| req.uri().authority().map(|a| a.as_str().to_owned()));
    let mut response = if !host.as_deref().is_some_and(|h| rule.allows(h)) {
        plain(StatusCode::MISDIRECTED_REQUEST, "This is not the dashboard's address.\n")
    } else if !method_allowed(req.method(), req.uri().path()) {
        let mut r = plain(StatusCode::METHOD_NOT_ALLOWED, "This dashboard only shows; it changes nothing.\n");
        let allow = if req.uri().path() == "/signin" { "GET, POST" } else { "GET" };
        r.headers_mut().insert(header::ALLOW, HeaderValue::from_static(allow));
        r
    } else {
        next.run(req).await
    };
    apply(response.headers_mut());
    response
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::any;
    use tower::ServiceExt;

    use super::*;

    fn app() -> Router {
        let rule = HostRule::new("127.0.0.1", 20130);
        Router::new().fallback(any(|| async { "page" })).layer(axum::middleware::from_fn_with_state(rule, enforce))
    }

    async fn call(method: &str, path: &str, host: Option<&str>) -> Response {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(h) = host {
            req = req.header(header::HOST, h);
        }
        app().oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
    }

    #[test]
    fn hosts_are_the_dashboards_own() {
        let rule = HostRule::new("127.0.0.2", 20130);
        for ok in ["127.0.0.1:20130", "localhost:20130", "LocalHost:20130", "[::1]:20130", "127.0.0.2:20130"] {
            assert!(rule.allows(ok), "{ok}");
        }
        for bad in [
            "evil.example:20130",
            "127.0.0.1:20129",
            "127.0.0.1",
            "localhost.evil.example:20130",
            "",
            "127.0.0.3:20130",
        ] {
            assert!(!rule.allows(bad), "{bad}");
        }
        assert!(HostRule::new("[::1]", 9).allows("[::1]:9"));
    }

    #[tokio::test]
    async fn every_response_carries_the_security_headers() {
        let r = call("GET", "/anything", Some("127.0.0.1:20130")).await;
        assert_eq!(r.status(), StatusCode::OK);
        let h = r.headers();
        assert_eq!(h[header::CONTENT_SECURITY_POLICY], CSP);
        assert!(!CSP.contains("script-src") && CSP.starts_with("default-src 'none'"));
        assert_eq!(h[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(h[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(h["cross-origin-resource-policy"], "same-origin");
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
    }

    #[tokio::test]
    async fn a_handlers_cache_control_is_kept_for_assets() {
        let rule = HostRule::new("127.0.0.1", 20130);
        let app = Router::new()
            .fallback(any(|| async { ([(header::CACHE_CONTROL, IMMUTABLE)], "asset") }))
            .layer(axum::middleware::from_fn_with_state(rule, enforce));
        let req =
            Request::builder().uri("/assets/x/y").header(header::HOST, "localhost:20130").body(Body::empty()).unwrap();
        let r = app.oneshot(req).await.unwrap();
        assert_eq!(r.headers()[header::CACHE_CONTROL], IMMUTABLE);
        assert_eq!(r.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    }

    #[tokio::test]
    async fn a_foreign_or_missing_host_is_421_and_no_page() {
        for host in [Some("evil.example:20130"), Some("127.0.0.1:1"), None] {
            let r = call("GET", "/quota", host).await;
            assert_eq!(r.status(), StatusCode::MISDIRECTED_REQUEST, "{host:?}");
            assert_eq!(r.headers()[header::X_FRAME_OPTIONS], "DENY", "refusals carry the headers too");
            let body = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
            assert!(!String::from_utf8_lossy(&body).contains("page"));
        }
        let r = call("DELETE", "/quota", Some("evil.example:20130")).await;
        assert_eq!(r.status(), StatusCode::MISDIRECTED_REQUEST, "Host is checked first");
    }

    #[tokio::test]
    async fn only_get_and_the_sign_in_post_pass() {
        let host = Some("127.0.0.1:20130");
        assert_eq!(call("GET", "/quota", host).await.status(), StatusCode::OK);
        assert_eq!(call("POST", "/signin", host).await.status(), StatusCode::OK);
        for (method, path) in [
            ("POST", "/quota"),
            ("POST", "/"),
            ("PUT", "/signin"),
            ("DELETE", "/quota"),
            ("PATCH", "/settings"),
            ("HEAD", "/quota"),
            ("OPTIONS", "/quota"),
            ("POST", "/signin/x"),
        ] {
            let r = call(method, path, host).await;
            assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
        }
        let r = call("DELETE", "/quota", host).await;
        assert_eq!(r.headers()[header::ALLOW], "GET");
        assert_eq!(call("PUT", "/signin", host).await.headers()[header::ALLOW], "GET, POST");
    }
}
