//! The dashboard's gate over real sockets (spec 009 US1, contracts/dashboard-http.md "Access"):
//! what opens before a token exists, what the token and its cookie open, what a wrong token
//! costs, and the Host, Origin and method rules that hold whatever the token.
//!
//! These test the gate, not the pages: a route "opens" when it isn't turned away, whatever the
//! page then says. The frame and each page are tested from Phase 4 on.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nullrouter_dashboard::spawn;
use nullrouter_engine::files::DashboardToken;
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::homes;
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::DashboardSettings;
use reqwest::header::{ALLOW, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, Url, redirect};
use tokio::sync::oneshot;

/// Every route the contract lists, an asset, a logo, and one it doesn't.
const ROUTES: [&str; 15] = [
    "/",
    "/endpoint",
    "/providers",
    "/providers/anthropic",
    "/combo",
    "/usage",
    "/usage/records/rq_1",
    "/quota",
    "/proxy-pools",
    "/console-log",
    "/settings",
    "/signin",
    "/assets/abc/style.css",
    "/logos/abc/anthropic.png",
    "/nope",
];

/// Routes the gate turns away without a valid cookie: all but the sign-in page and the assets.
fn gated() -> impl Iterator<Item = &'static str> {
    ROUTES.into_iter().filter(|p| *p != "/signin" && !p.starts_with("/assets/"))
}

const NO_TOKEN_TEXT: &str = "nullrouter dashboard token";
const WRONG_TOKEN_TEXT: &str = "That token is not the current one.";

/// A dashboard on a free loopback port, over a home with no token.
struct Dash {
    dir: tempfile::TempDir,
    engine: Arc<Engine>,
    addr: SocketAddr,
    http: Client,
    _stop: oneshot::Sender<()>,
}

impl Dash {
    async fn start() -> Self {
        let dir = homes::empty();
        let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
        let engine = Arc::new(engine);
        let settings = DashboardSettings { enabled: true, listen: "127.0.0.1:0".into() };
        let (stop, stopped) = oneshot::channel::<()>();
        let handle = spawn(engine.clone(), &settings, "test", async move {
            let _ = stopped.await;
        })
        .await;
        let addr = handle.addr().expect("the dashboard bound a loopback port");
        let http = Client::builder()
            .redirect(redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(40))
            .build()
            .unwrap();
        Self { dir, engine, addr, http, _stop: stop }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Issues `token` as the CLI would: the digest to `dashboard.toml`, then a reload.
    async fn issue(&self, token: &str) {
        DashboardToken { digest: Some(DashboardToken::digest_of(token)), issued: Some("2026-10-06T08:00:00Z".into()) }
            .save(self.dir.path())
            .unwrap();
        self.engine.reload().await.unwrap();
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        self.http.request(method, format!("http://{}{path}", self.addr))
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.req(Method::GET, path)
    }

    /// `POST /signin` with `token=…`; `next`, when given, goes in the body and the query, as the
    /// contract doesn't say which carries it.
    fn sign_in(&self, token: &str, next: Option<&str>) -> RequestBuilder {
        let mut body = format!("token={}", form(token));
        let mut path = "/signin".to_owned();
        if let Some(next) = next {
            body.push_str(&format!("&next={}", form(next)));
            path.push_str(&format!("?next={}", form(next)));
        }
        self.req(Method::POST, &path).header(CONTENT_TYPE, "application/x-www-form-urlencoded").body(body)
    }
}

/// A token as the CLI makes them: `nrd_` and 43 characters.
fn token(n: u8) -> String {
    format!("nrd_{:0>43}", format!("fixture{n}"))
}

fn cookie(token: &str) -> String {
    format!("nr_dashboard={token}")
}

/// Percent-encodes everything but unreserved characters.
fn form(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn location(r: &Response) -> &str {
    r.headers().get(LOCATION).and_then(|v| v.to_str().ok()).unwrap_or_default()
}

/// The `next` a sign-in redirect carries, decoded; it must be the only parameter.
fn next_of(r: &Response) -> String {
    let loc = location(r);
    let url = Url::parse(&format!("http://dashboard{loc}")).unwrap();
    assert_eq!(url.path(), "/signin", "{loc}");
    let pairs: Vec<_> = url.query_pairs().collect();
    assert_eq!(pairs.len(), 1, "one parameter in {loc}");
    assert_eq!(pairs[0].0, "next", "{loc}");
    pairs[0].1.to_string()
}

fn is_sign_in_redirect(r: &Response) -> bool {
    r.status() == StatusCode::SEE_OTHER && location(r).starts_with("/signin")
}

/// Not turned away by the gate (the page behind it may not exist yet).
fn passes_the_gate(r: &Response) -> bool {
    !is_sign_in_redirect(r)
        && ![StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN, StatusCode::METHOD_NOT_ALLOWED].contains(&r.status())
        && r.status() != StatusCode::MISDIRECTED_REQUEST
}

#[tokio::test(flavor = "multi_thread")]
async fn before_a_token_exists_only_the_sign_in_page_opens_and_it_names_the_command() {
    let d = Dash::start().await;

    let r = d.get("/signin").send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let page = r.text().await.unwrap();
    assert!(page.contains(NO_TOKEN_TEXT), "{page}");
    assert!(!page.contains("name=\"token\""), "no form while there is no token to enter");

    for path in gated() {
        let r = d.get(path).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "{path}");
        assert_eq!(location(&r), "/signin", "{path}");
    }

    // The sign-in post has nothing to check a token against.
    let r = d.sign_in(&token(1), None).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers().get(SET_COOKIE).is_none());
    assert!(r.text().await.unwrap().contains(NO_TOKEN_TEXT));

    // Assets carry nothing of 0router's and need no cookie: not turned to the sign-in page.
    let r = d.get("/assets/abc/style.css").send().await.unwrap();
    assert!(!is_sign_in_redirect(&r), "{}", r.status());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_right_token_sets_a_long_lived_cookie_and_the_cookie_opens_pages() {
    let d = Dash::start().await;
    let t = token(1);
    d.issue(&t).await;

    // With a token issued, the sign-in page is the form.
    let page = d.get("/signin").send().await.unwrap().text().await.unwrap();
    assert!(page.contains("name=\"token\""), "{page}");
    assert!(!page.contains(NO_TOKEN_TEXT), "{page}");

    // No cookie: every gated route sends the browser to sign in and back (`next` is path and
    // query, in one parameter however many the query has).
    for (path, next) in [
        ("/endpoint", "/endpoint"),
        ("/providers?q=a", "/providers?q=a"),
        ("/usage?before=rq_1&notices", "/usage?before=rq_1&notices"),
    ] {
        let r = d.get(path).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "{path}");
        assert_eq!(next_of(&r), next, "{path}");
    }
    for path in gated() {
        let r = d.get(path).send().await.unwrap();
        assert!(is_sign_in_redirect(&r), "{path}: {}", r.status());
    }

    // A wrong cookie is no cookie.
    let r = d.get("/endpoint").header(COOKIE, cookie(&token(2))).send().await.unwrap();
    assert!(is_sign_in_redirect(&r), "{}", r.status());

    // The right token: back to `/`, and the cookie the contract spells out.
    let r = d.sign_in(&t, None).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/");
    let set = r.headers().get(SET_COOKIE).expect("a cookie").to_str().unwrap();
    assert_eq!(set, format!("nr_dashboard={t}; Path=/; HttpOnly; SameSite=Strict; Max-Age=34560000"));

    // With it, every route is open; the sign-in page and a repeated post go home.
    for path in gated() {
        let r = d.get(path).header(COOKIE, cookie(&t)).send().await.unwrap();
        assert!(passes_the_gate(&r), "{path}: {}", r.status());
    }
    let r = d.get("/signin").header(COOKIE, cookie(&t)).send().await.unwrap();
    assert_eq!((r.status(), location(&r)), (StatusCode::SEE_OTHER, "/"));
    let r = d.sign_in("whatever", None).header(COOKIE, cookie(&t)).send().await.unwrap();
    assert_eq!((r.status(), location(&r)), (StatusCode::SEE_OTHER, "/"));

    // Logos are behind the cookie like a page (a user plugin's logo names the plugin); assets are not.
    let r = d.get("/logos/abc/anthropic.png").send().await.unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(next_of(&r), "/logos/abc/anthropic.png");
    let r = d.get("/assets/abc/style.css").send().await.unwrap();
    assert!(!is_sign_in_redirect(&r), "{}", r.status());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_token_waits_and_the_wait_doubles_until_a_success_resets_it() {
    let d = Dash::start().await;
    let right = token(1);
    d.issue(&right).await;

    let mut waits = Vec::new();
    for _ in 0..2 {
        let started = Instant::now();
        let r = d.sign_in(&token(9), None).send().await.unwrap();
        waits.push(started.elapsed());
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert!(r.headers().get(SET_COOKIE).is_none());
        let page = r.text().await.unwrap();
        assert!(page.contains(WRONG_TOKEN_TEXT), "{page}");
        assert!(page.contains("name=\"token\""), "the form is offered again: {page}");
    }
    assert!(waits[0] >= Duration::from_secs(1), "first wait {:?}", waits[0]);
    assert!(waits[0] < Duration::from_secs(5), "first wait {:?}", waits[0]);
    assert!(waits[1] > waits[0].mul_f32(1.5), "the wait doubles: {waits:?}");

    let r = d.sign_in(&right, None).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    let started = Instant::now();
    let r = d.sign_in(&token(9), None).send().await.unwrap();
    let after = started.elapsed();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert!(after < waits[1].mul_f32(0.75), "a success resets the wait: {waits:?} then {after:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_token_signs_the_old_cookie_out() {
    let d = Dash::start().await;
    let (first, second) = (token(1), token(2));
    d.issue(&first).await;
    let r = d.get("/endpoint").header(COOKIE, cookie(&first)).send().await.unwrap();
    assert!(passes_the_gate(&r), "{}", r.status());

    d.issue(&second).await;
    let r = d.get("/endpoint").header(COOKIE, cookie(&first)).send().await.unwrap();
    assert!(is_sign_in_redirect(&r), "{}", r.status());

    let r = d.sign_in(&second, None).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(r.headers()[SET_COOKIE].to_str().unwrap().starts_with(&cookie(&second)));
    let r = d.get("/endpoint").header(COOKIE, cookie(&second)).send().await.unwrap();
    assert!(passes_the_gate(&r), "{}", r.status());
}

#[tokio::test(flavor = "multi_thread")]
async fn next_outside_the_dashboard_becomes_the_root() {
    let d = Dash::start().await;
    let t = token(1);
    d.issue(&t).await;
    for (given, lands) in [
        ("/providers?q=a", "/providers?q=a"),
        ("//evil.example", "/"),
        ("https://evil.example/x", "/"),
        ("evil.example", "/"),
        ("/\\evil.example", "/"),
        ("", "/"),
    ] {
        let r = d.sign_in(&t, Some(given)).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "next={given:?}");
        assert_eq!(location(&r), lands, "next={given:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_foreign_origin_cannot_post_to_sign_in() {
    let d = Dash::start().await;
    let t = token(1);
    d.issue(&t).await;
    let port = d.port();
    for origin in ["http://evil.example", "null", &format!("http://127.0.0.1:{}", port + 1)] {
        let r = d.sign_in(&t, None).header("Origin", origin).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "Origin: {origin}");
        assert!(r.headers().get(SET_COOKIE).is_none(), "Origin: {origin}");
    }
    // This dashboard's own origins, and no Origin at all (curl), sign in.
    for origin in [Some(format!("http://127.0.0.1:{port}")), Some(format!("http://localhost:{port}")), None] {
        let mut req = d.sign_in(&t, None);
        if let Some(o) = &origin {
            req = req.header("Origin", o);
        }
        let r = req.send().await.unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "Origin: {origin:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_foreign_host_gets_421_and_no_page() {
    let d = Dash::start().await;
    let t = token(1);
    d.issue(&t).await;
    let port = d.port();
    for host in ["evil.example", &format!("evil.example:{port}"), &format!("127.0.0.1:{}", port + 1)] {
        for req in [d.get("/signin"), d.get("/endpoint").header(COOKIE, cookie(&t)), d.sign_in(&t, None)] {
            let r = req.header("Host", host).send().await.unwrap();
            assert_eq!(r.status(), StatusCode::MISDIRECTED_REQUEST, "Host: {host}");
            assert!(r.headers()[CONTENT_TYPE].to_str().unwrap().starts_with("text/plain"));
            let body = r.text().await.unwrap();
            assert!(!body.contains("<html") && !body.contains("<form"), "{body}");
        }
    }
    // The dashboard's own names, in any case.
    for host in [format!("localhost:{port}"), format!("LOCALHOST:{port}"), format!("[::1]:{port}")] {
        let r = d.get("/signin").header("Host", &host).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "Host: {host}");
    }
}

/// FR-013: nothing here changes state. Every route with every method but GET is 405, signed in
/// or not; the one exception is `POST /signin`.
#[tokio::test(flavor = "multi_thread")]
async fn no_write_routes() {
    let d = Dash::start().await;
    let t = token(1);
    for issued in [false, true] {
        if issued {
            d.issue(&t).await;
        }
        for path in ROUTES {
            for method in
                [Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::HEAD, Method::OPTIONS, Method::TRACE]
            {
                if method == Method::POST && path == "/signin" {
                    continue;
                }
                let mut req = d.req(method.clone(), path);
                if issued {
                    req = req.header(COOKIE, cookie(&t));
                }
                let r = req.send().await.unwrap();
                assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED, "{method} {path} (token issued: {issued})");
                let allow = r.headers().get(ALLOW).and_then(|v| v.to_str().ok()).unwrap_or_default();
                assert!(allow.contains("GET"), "{method} {path}: Allow: {allow:?}");
            }
        }
    }
}
