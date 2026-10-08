//! What the page suites share (T032): a real `serve` (client listener, engine, dashboard) on a
//! fixture home, signed in; a page fetched as a browser would; and the view the CLI's `--json`
//! prints for the same moment, built by the same function in process (research R1).
//!
//! "Agreement" means every scalar the CLI's JSON holds for a fact the page shows appears in the
//! page: text compared after tags are stripped and entities decoded, instants compared through
//! `<time datetime>`.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use nullrouter_dashboard::page::{self, ViewName, Want};
use nullrouter_dashboard::spawn;
use nullrouter_engine::files::DashboardToken;
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::homes;
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::DashboardSettings;
use nullrouter_server::serve::{self, App};
use reqwest::header::COOKIE;
use reqwest::{Client, StatusCode, redirect};
use serde_json::Value;
use tokio::sync::oneshot;

/// The token every suite signs in with.
pub const TOKEN: &str = "nrd_fixture00000000000000000000000000000000000";

/// A served home, signed in.
pub struct Dash {
    pub dir: tempfile::TempDir,
    pub engine: Arc<Engine>,
    pub addr: SocketAddr,
    pub client_addr: SocketAddr,
    http: Client,
    _stops: (oneshot::Sender<()>, oneshot::Sender<()>),
}

impl Dash {
    /// `serve` on the `dashboard()` fixture home (every status of User Stories 2 to 6).
    pub async fn dashboard() -> Self {
        Self::start(homes::dashboard()).await
    }

    /// `serve` on a home with nothing in it.
    pub async fn empty() -> Self {
        Self::start(homes::empty()).await
    }

    /// `serve` on `dir`: the client listener first (its address is the endpoint), then the
    /// dashboard on a free loopback port; then a signed-in browser. The token is written before
    /// the engine opens: the `dashboard()` home holds an invalid plugin, so a reload refuses it.
    pub async fn start(dir: tempfile::TempDir) -> Self {
        DashboardToken { digest: Some(DashboardToken::digest_of(TOKEN)), issued: Some("2026-10-06T08:00:00Z".into()) }
            .save(dir.path())
            .unwrap();
        let (engine, _) = Engine::open(OperatorHome::new(dir.path())).expect("the fixture home opens");
        let engine = Arc::new(engine);

        let client = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client_addr = client.local_addr().unwrap();
        engine.status.set_client_listen(client_addr.to_string());
        let app = App::new(engine.clone()).unwrap();
        let (client_stop, client_stopped) = oneshot::channel::<()>();
        tokio::spawn(serve::run(app, client, async {
            let _ = client_stopped.await;
        }));

        let settings = DashboardSettings { enabled: true, listen: "127.0.0.1:0".into() };
        let (stop, stopped) = oneshot::channel::<()>();
        let handle = spawn(engine.clone(), &settings, "nullrouter 0.1.0", async move {
            let _ = stopped.await;
        })
        .await;
        let addr = handle.addr().expect("the dashboard bound a loopback port");

        let http = Client::builder()
            .redirect(redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();
        Self { dir, engine, addr, client_addr, http, _stops: (stop, client_stop) }
    }

    pub fn home(&self) -> OperatorHome {
        OperatorHome::new(self.dir.path())
    }

    /// `GET path` with the cookie: the status and the body.
    pub async fn page(&self, path: &str) -> (StatusCode, String) {
        let r = self
            .http
            .get(format!("http://{}{path}", self.addr))
            .header(COOKIE, format!("nr_dashboard={TOKEN}"))
            .send()
            .await
            .unwrap();
        let status = r.status();
        (status, r.text().await.unwrap())
    }

    /// `GET path`, which must be a 200 page; its HTML.
    pub async fn ok(&self, path: &str) -> String {
        let (status, body) = self.page(path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        body
    }

    /// What the CLI's `--json` prints for `view` with `args` (its command-line arguments), read
    /// from the same running engine.
    pub async fn view(&self, view: ViewName, args: Value) -> Value {
        self.view_full(view, args).await.0
    }

    /// The view's `json` and `extra`.
    pub async fn view_full(&self, view: ViewName, args: Value) -> (Value, Value) {
        let p = page::build(&self.engine, &[Want::new(view, args)]).await.expect("the view builds");
        let v = &p.views[0].value;
        (v.json.clone(), v.extra.clone())
    }
}

/// The page's visible text: tags removed, entities decoded, whitespace collapsed.
pub fn text_of(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = decode(&out);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decodes the entities maud writes.
pub fn decode(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

/// Every `datetime="…"` on the page.
pub fn times(html: &str) -> Vec<String> {
    attrs(html, "datetime")
}

/// Every value of the attribute `name` on the page, decoded.
pub fn attrs(html: &str, name: &str) -> Vec<String> {
    let pat = format!(" {name}=\"");
    html.match_indices(&pat)
        .filter_map(|(i, _)| {
            let rest = &html[i + pat.len()..];
            rest.find('"').map(|end| decode(&rest[..end]))
        })
        .collect()
}

/// Whether `rfc3339` is an instant some `<time datetime>` on the page shows.
pub fn shows_instant(html: &str, rfc3339: &str) -> bool {
    let want: jiff::Timestamp = match rfc3339.parse() {
        Ok(t) => t,
        Err(_) => return false,
    };
    times(html).iter().any(|t| t.parse::<jiff::Timestamp>().is_ok_and(|t| t == want))
}

/// Asserts the page shows `value`: an RFC 3339 instant through `<time datetime>`, anything else
/// in its text (`what` names the fact in the failure).
pub fn assert_shows(html: &str, value: &Value, what: &str) {
    let text = text_of(html);
    match value {
        Value::Null => {}
        Value::String(s) if s.parse::<jiff::Timestamp>().is_ok() => {
            assert!(shows_instant(html, s), "{what}: the instant {s} is not on the page\n{text}");
        }
        Value::String(s) => {
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(text.contains(&s), "{what}: {s:?} is not on the page\n{text}");
        }
        Value::Number(n) => assert!(text.contains(&n.to_string()), "{what}: {n} is not on the page\n{text}"),
        Value::Bool(_) => {}
        Value::Array(a) => a.iter().for_each(|v| assert_shows(html, v, what)),
        Value::Object(o) => o.iter().for_each(|(k, v)| assert_shows(html, v, &format!("{what}.{k}"))),
    }
}

/// Asserts each of `keys` of `object` is on the page.
pub fn assert_fields(html: &str, object: &Value, keys: &[&str], what: &str) {
    for k in keys {
        assert_shows(html, &object[*k], &format!("{what}.{k}"));
    }
}

/// The `<section class="card">` blocks of the page, in order (a card's text and its HTML).
pub fn cards(html: &str) -> Vec<String> {
    html.split("<section class=\"card")
        .skip(1)
        .map(|s| s.split("</section>").next().unwrap_or_default().to_owned())
        .collect()
}

/// The fixture home plus three requests a minute ago: two by `ak_fixture1` on `xai`, one by
/// `ak_fixture2` on `anthropic`, so the last 24 hours have traffic to draw.
pub fn home_with_traffic() -> tempfile::TempDir {
    let dir = nullrouter_engine::testkit::homes::dashboard();
    let now = jiff::Timestamp::now() - jiff::SignedDuration::from_secs(60);
    let at = now.to_string();
    let day = &at[..10];
    let mut text = String::new();
    for (n, (agent, provider, started, ttft)) in [
        ("ak_fixture1", "xai", 4.0, 300.0),
        ("ak_fixture1", "xai", 6.0, 1900.0),
        ("ak_fixture2", "anthropic", 5.0, 640.0),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("rq_land{n}");
        for line in [
            json!({"v":1,"t":"open","id":id,"arrived":at,"agent":agent,"style":"anthropic-messages","op":"generate","type":"text","target":"t"}),
            json!({"v":1,"t":"attempt","id":id,"attempt":{"n":1,"provider":provider,"account":"main","model":"m","kind":"initial","started":started,"ended":2000.0,"outcome":{"state":"ok"},"dropped":[],"forced":[]}}),
            json!({"v":1,"t":"close","id":id,"outcome":"succeeded","served_by":{"provider":provider,"account":"main","model":"m"},"ttft_ms":ttft,"total_ms":2000.0,"usage":null,"break_handling":{"kind":"none"},"job":null}),
        ] {
            text += &(line.to_string() + "\n");
        }
    }
    let path = dir.path().join("records").join(format!("{day}.jsonl"));
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(path, old + &text).unwrap();
    dir
}
