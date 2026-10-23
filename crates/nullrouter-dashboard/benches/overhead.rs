//! Spec 014 SC-006 (T046): router overhead while the browser makes changes as fast as it can.
//!
//! One server (client listener, dashboard, instant mock upstream) is measured in four conditions,
//! in this order, in the same run: A idle; C ONE poster posting dashboard changes sequentially as
//! fast as it can (the spec's "browser as fast as it can"); B `POSTERS` posters (stress, not a
//! gate); A2 idle again to show the drift between two idle conditions. The posts are
//! `routing.window` set/clear and `behaviour.set_break`, each of which rewrites `config.toml` and
//! reloads the engine. The client and the posters run on their own one-thread runtimes, so the
//! bench measures the server. Each condition sends `REQUESTS` whole and `REQUESTS` streamed chat
//! requests; the per-request latency (stream: first chunk) and the router overhead from the
//! request records (`summary::router_overhead`) are reported at the median and p95.
//!
//! Verdict: C p95 no more than 1.10 times A p95. Baseline: `specs/014-functional-dashboard/bench-baseline.md`.
//!
//! This bench lives in the dashboard crate because the signed-in site, the form token and the
//! post wiring are reachable here and the dashboard already depends on the server and engine.
//!
//! `cargo bench -p nullrouter-dashboard --bench overhead`

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use nullrouter_dashboard::access::encode_component;
use nullrouter_dashboard::spawn;
use nullrouter_engine::files::{DashboardToken, write_private};
use nullrouter_engine::journal::{records, summary};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::DashboardSettings;
use nullrouter_server::serve::{self, App};
use reqwest::header::{CONTENT_TYPE, COOKIE, ORIGIN};
use reqwest::{Client, StatusCode, redirect};
use serde_json::{Value, json};
use tokio::sync::oneshot;

const TOKEN: &str = "nrd_bench0000000000000000000000000000000000000000";
const REQUESTS: usize = 2500;
const POSTERS: usize = 4;

fn chat_whole() -> Step {
    Step::json(
        200,
        json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi there"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}}),
    )
}

fn chat_stream() -> Step {
    let chunk = |delta: Value, finish: Value| {
        (
            None,
            json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}),
        )
    };
    Step::sse(
        &[
            chunk(json!({"role": "assistant", "content": "Hel"}), Value::Null),
            chunk(json!({"content": "lo"}), Value::Null),
            chunk(json!({}), json!("stop")),
        ],
        true,
    )
}

fn pct(all: &mut [Duration], q: f64) -> Duration {
    all.sort_unstable();
    all[((all.len() - 1) as f64 * q).round() as usize]
}

fn pct_f(all: &mut [f64], q: f64) -> f64 {
    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    all[((all.len() - 1) as f64 * q).round() as usize]
}

struct Setup {
    _dir: tempfile::TempDir,
    engine: Arc<Engine>,
    client: String,
    key: String,
    dash: SocketAddr,
    cookie: String,
    mock: MockUpstream,
    _stops: (oneshot::Sender<()>, oneshot::Sender<()>),
}

async fn setup() -> Setup {
    let mock = MockUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::write(home.join("config.toml"), "allow_private_endpoints = true\n").unwrap();
    std::fs::create_dir(home.join("plugins")).unwrap();
    std::fs::write(
        home.join("plugins/mockco.toml"),
        format!(
            "schema = 2\nid = \"mockco\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
            mock.url("/v1/chat/completions")
        ),
    )
    .unwrap();
    write_private(
        &home.join(nullrouter_engine::accounts::FILE),
        "schema = 1\n[[account]]\nprovider = \"mockco\"\nname = \"main\"\nsecret = \"sk-mock-bench\"\n",
    )
    .unwrap();
    let mut ks = Keys::default();
    let (key, _) = ks.issue("laptop", None).unwrap();
    write_private(&home.join(keys::FILE), &ks.to_toml()).unwrap();
    DashboardToken { digest: Some(DashboardToken::digest_of(TOKEN)), issued: Some("2026-10-06T08:00:00Z".into()) }
        .save(home)
        .unwrap();

    let (engine, _) = Engine::open(OperatorHome::new(home)).expect("the bench home opens");
    let engine = Arc::new(engine);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client_addr = listener.local_addr().unwrap();
    engine.status.set_client_listen(client_addr.to_string());
    let app = App::new(engine.clone()).unwrap();
    let (client_stop, client_stopped) = oneshot::channel::<()>();
    serve::recover_journal(&engine).await;
    tokio::spawn(serve::serve_recovered(app, listener, async {
        let _ = client_stopped.await;
    }));
    let settings = DashboardSettings { enabled: true, listen: "127.0.0.1:0".into() };
    let (stop, stopped) = oneshot::channel::<()>();
    let handle = spawn(engine.clone(), &settings, "nullrouter bench", async move {
        let _ = stopped.await;
    })
    .await;
    let dash = handle.addr().expect("the dashboard bound a port");

    let http = Client::builder().redirect(redirect::Policy::none()).no_proxy().build().unwrap();
    let r = http
        .post(format!("http://{dash}/signin"))
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!("token={TOKEN}&next=%2F"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER, "the bench signs in");
    let cookie = r.headers()[reqwest::header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_owned();
    mock.respond(|r| if r.json()["stream"] == true { chat_stream() } else { chat_whole() });
    Setup {
        _dir: dir,
        engine,
        client: format!("http://{client_addr}"),
        key,
        dash,
        cookie,
        mock,
        _stops: (stop, client_stop),
    }
}

/// One post to `/do/<route>` with a fresh form token, as the page's form does. Returns whether
/// the dashboard answered with a redirect.
async fn post(http: &Client, s: &(SocketAddr, String), route: &str, fields: &[(&str, &str)]) -> bool {
    let (addr, cookie) = s;
    let token = http
        .get(format!("http://{addr}/debug/form-token"))
        .header(COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let mut body = format!("_t={token}&_back={}", encode_component("/settings"));
    for (k, v) in fields {
        body.push_str(&format!("&{k}={}", encode_component(v)));
    }
    let r = http
        .post(format!("http://{addr}/do/{route}"))
        .header(COOKIE, cookie)
        .header(ORIGIN, format!("http://{addr}"))
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .unwrap();
    r.status() == StatusCode::SEE_OTHER
}

struct Stats {
    whole: Vec<Duration>,
    stream: Vec<Duration>,
    overhead: Vec<f64>,
    posts: u64,
    secs: f64,
}

async fn condition(
    s: &Setup,
    posters: usize,
    rts: &(tokio::runtime::Handle, tokio::runtime::Handle),
    seen: &mut std::collections::HashSet<String>) -> Stats {
    let stop = Arc::new(AtomicBool::new(false));
    let posts = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::new();
    for n in 0..posters {
        let (stop, posts) = (stop.clone(), posts.clone());
        let site = (s.dash, s.cookie.clone());
        tasks.push(rts.1.spawn(async move {
            let http = Client::builder().redirect(redirect::Policy::none()).no_proxy().build().unwrap();
            let mut i = n;
            while !stop.load(Ordering::Relaxed) {
                let ok = if i % 2 == 0 {
                    let d = ["6h", "default"][(i / 2) % 2];
                    post(&http, &site, "routing/window", &[("duration", d)]).await
                } else {
                    let b = ["error_event", "restart"][(i / 2) % 2];
                    post(&http, &site, "behaviour/set-break", &[("behaviour", b)]).await
                };
                assert!(ok, "a post was not redirected");
                posts.fetch_add(1, Ordering::Relaxed);
                i += 1;
            }
        }));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (curl, ckey) = (format!("{}/v1/chat/completions", s.client), s.key.clone());
    let started = Instant::now();
    let (whole, stream) = rts.0.spawn(async move {
    let client = Client::new();
    let url = curl;
    let (mut whole, mut stream) = (Vec::new(), Vec::new());
    for n in 0..REQUESTS * 2 {
        let streamed = n % 2 == 1;
        let body = json!({"model": "mockco/m1", "stream": streamed, "messages": [{"role": "user", "content": "hi"}]});
        let t = Instant::now();
        let mut r = client.post(&url).bearer_auth(&ckey).body(body.to_string()).send().await.unwrap();
        assert_eq!(r.status(), 200);
        if streamed {
            r.chunk().await.unwrap();
            stream.push(t.elapsed());
            while r.chunk().await.unwrap().is_some() {}
        } else {
            r.bytes().await.unwrap();
            whole.push(t.elapsed());
        }
    }
    (whole, stream)
    }).await.unwrap();
    let secs = started.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    for t in tasks {
        t.await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut overhead = Vec::new();
    for r in records::read(s.engine.home().path(), &records::Filter::default()) {
        let id = r["id"].as_str().unwrap_or_default().to_owned();
        if seen.insert(id)
            && let Some(o) = summary::router_overhead(&r)
        {
            overhead.push(o);
        }
    }
    Stats { whole, stream, overhead, posts: posts.load(Ordering::Relaxed), secs }
}

fn show(name: &str, st: &mut Stats) -> [f64; 3] {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let (w50, w95) = (ms(pct(&mut st.whole, 0.5)), ms(pct(&mut st.whole, 0.95)));
    let (s50, s95) = (ms(pct(&mut st.stream, 0.5)), ms(pct(&mut st.stream, 0.95)));
    let (o50, o95) = if st.overhead.is_empty() {
        (f64::NAN, f64::NAN)
    } else {
        (pct_f(&mut st.overhead, 0.5), pct_f(&mut st.overhead, 0.95))
    };
    println!(
        "{name}: whole median {w50:.3} ms p95 {w95:.3} ms | stream_ttfb median {s50:.3} ms p95 {s95:.3} ms | router_overhead median {o50:.3} ms p95 {o95:.3} ms ({} records) | {} posts in {:.1} s = {:.1}/s",
        st.overhead.len(),
        st.posts,
        st.secs,
        st.posts as f64 / st.secs
    );
    [w95, s95, o95]
}

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    // The client and the posters get their own runtimes so the bench process does not take the
    // server's worker threads.
    let crt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let prt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let rts = (crt.handle().clone(), prt.handle().clone());
    rt.block_on(async {
        let s = setup().await;
        let mut seen = std::collections::HashSet::new();
        let _ = condition(&s, 0, &rts, &mut seen).await; // warm up
        let mut a = condition(&s, 0, &rts, &mut seen).await;
        let mut c = condition(&s, 1, &rts, &mut seen).await;
        let mut b = condition(&s, POSTERS, &rts, &mut seen).await;
        let mut a2 = condition(&s, 0, &rts, &mut seen).await;
        let a = show("A  idle      ", &mut a);
        let c = show("C  1 poster  ", &mut c);
        let b = show(&format!("B  {POSTERS} posters "), &mut b);
        let a2 = show("A2 idle      ", &mut a2);
        for (i, what) in ["whole", "stream_ttfb", "router_overhead"].into_iter().enumerate() {
            println!(
                "{what}: C/A p95 = {:.3} -> {} | B/A = {:.3} (stress, not a gate) | A2/A = {:.3}",
                c[i] / a[i],
                if c[i] / a[i] <= 1.10 { "within 10%" } else { "OVER 10%" },
                b[i] / a[i],
                a2[i] / a[i]
            );
        }
        let _ = &s.mock;
    });
}
