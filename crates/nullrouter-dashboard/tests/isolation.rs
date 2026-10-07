//! The dashboard never slows client traffic (T071, SC-005, FR-016): 1,000 client requests to a
//! mock provider, first with `enabled = false`, then while four workers load pages, then with
//! every page build panicking, then with every build sleeping 20 s. Each loaded run's client p95
//! is at most 1 ms above the baseline's and it has no failure the baseline didn't.
//!
//! Ignored by default and built only with the `fault` feature; run it in release:
//! `cargo test -p nullrouter-dashboard --release --features fault --test isolation -- --ignored`.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nullrouter_dashboard::page::{Fault, set_fault};
use nullrouter_dashboard::pages::Id;
use nullrouter_dashboard::spawn;
use nullrouter_engine::files::{DashboardToken, write_private};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::{MockUpstream, Step, homes};
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::DashboardSettings;
use nullrouter_server::serve::{self, App};
use reqwest::Client;
use reqwest::header::COOKIE;
use serde_json::json;
use tokio::sync::oneshot;

const TOKEN: &str = "nrd_isolation0000000000000000000000000000000000";
const REQUESTS: usize = 1_000;
const WORKERS: usize = 4;

/// The `dashboard()` home with a provider at the mock and an agent key, returned whole.
fn home(mock: &MockUpstream) -> (tempfile::TempDir, String) {
    let dir = homes::dashboard();
    let h = dir.path();
    fs::write(
        h.join("plugins/iso.toml"),
        format!(
            "schema = 2\nid = \"iso\"\ncategory = \"apikey\"\n\n[auth]\nkind = \"apikey\"\nheader = \"Authorization\"\n\
             scheme = \"bearer\"\n\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n\n[[models]]\nid = \"m1\"\n",
            mock.url("/iso/chat/completions")
        ),
    )
    .unwrap();
    let config = fs::read_to_string(h.join("config.toml")).unwrap();
    fs::write(h.join("config.toml"), format!("allow_private_endpoints = true\n{config}")).unwrap();
    let file = h.join(nullrouter_engine::accounts::FILE);
    let accounts = fs::read_to_string(&file).unwrap();
    write_private(&file, &format!("{accounts}\n[[account]]\nprovider = \"iso\"\nname = \"main\"\nsecret = \"sk-iso-0001\"\norder = 9\n"))
        .unwrap();
    let mut keys = Keys::load(&h.join(keys::FILE)).unwrap();
    let (agent, _) = keys.issue("iso-agent", None).unwrap();
    write_private(&h.join(keys::FILE), &keys.to_toml()).unwrap();
    DashboardToken { digest: Some(DashboardToken::digest_of(TOKEN)), issued: Some("2026-10-07T08:00:00Z".into()) }
        .save(h)
        .unwrap();
    (dir, agent)
}

/// The client p95 and the failures of [`REQUESTS`] requests, with the dashboard on or off and,
/// when on, [`WORKERS`] workers loading every page meanwhile.
async fn run(mock: &MockUpstream, enabled: bool, fault: Fault) -> (Duration, usize) {
    set_fault(fault);
    let (dir, agent) = home(mock);
    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).expect("the home opens");
    let engine = Arc::new(engine);
    let client = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client_addr = client.local_addr().unwrap();
    engine.status.set_client_listen(client_addr.to_string());
    let (client_stop, client_stopped) = oneshot::channel::<()>();
    tokio::spawn(serve::run(App::new(engine.clone()).unwrap(), client, async {
        let _ = client_stopped.await;
    }));
    let settings = DashboardSettings { enabled, listen: "127.0.0.1:0".into() };
    let (stop, stopped) = oneshot::channel::<()>();
    let handle = spawn(engine.clone(), &settings, "nullrouter 0.1.0", async move {
        let _ = stopped.await;
    })
    .await;

    let done = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    if let Some(addr) = handle.addr() {
        for w in 0..WORKERS {
            let done = done.clone();
            workers.push(tokio::spawn(async move {
                let http = Client::builder().no_proxy().timeout(Duration::from_secs(30)).build().unwrap();
                let mut i = w;
                while !done.load(Ordering::Relaxed) {
                    let path = Id::ALL[i % Id::ALL.len()].path();
                    let req = http.get(format!("http://{addr}{path}")).header(COOKIE, format!("nr_dashboard={TOKEN}"));
                    let _ = tokio::time::timeout(Duration::from_secs(1), req.send()).await;
                    i += 1;
                }
            }));
        }
    }

    let http = Client::builder().no_proxy().timeout(Duration::from_secs(30)).build().unwrap();
    let body = json!({"model": "iso/m1", "messages": [{"role": "user", "content": "hi"}]}).to_string();
    let (mut took, mut failures) = (Vec::with_capacity(REQUESTS), 0);
    for _ in 0..REQUESTS {
        let start = Instant::now();
        let r = http
            .post(format!("http://{client_addr}/v1/chat/completions"))
            .bearer_auth(&agent)
            .body(body.clone())
            .send()
            .await;
        let ok = match r {
            Ok(r) => r.status().is_success() && r.bytes().await.is_ok(),
            Err(_) => false,
        };
        took.push(start.elapsed());
        failures += usize::from(!ok);
    }
    done.store(true, Ordering::Relaxed);
    for w in workers {
        w.abort();
    }
    let _ = (stop.send(()), client_stop.send(()));
    set_fault(Fault::None);
    took.sort();
    (took[REQUESTS * 95 / 100], failures)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release-mode load run: --release --features fault -- --ignored"]
async fn page_loads_and_faulty_builds_leave_client_traffic_alone() {
    let mock = MockUpstream::start().await;
    mock.respond(|_| {
        Step::json(
            200,
            json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "m1",
                   "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 1, "completion_tokens": 1}}),
        )
    });
    let (base_p95, base_failures) = run(&mock, false, Fault::None).await;
    for (what, fault) in [("pages loading", Fault::None), ("every build panics", Fault::Panic), ("every build sleeps 20 s", Fault::Sleep)] {
        let (p95, failures) = run(&mock, true, fault).await;
        println!("{what}: p95 {p95:?} (baseline {base_p95:?}), failures {failures} (baseline {base_failures})");
        assert!(p95 <= base_p95 + Duration::from_millis(1), "{what}: p95 {p95:?} against {base_p95:?}");
        assert!(failures <= base_failures, "{what}: {failures} failures against {base_failures}");
    }
}
