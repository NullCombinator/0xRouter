//! The dashboard's listener (spec 009 US1, FR-002, SC-004): a taken port is reported and never
//! stops clients; `enabled = false` opens no port; the bound address is loopback and not
//! reachable on another address of this host.
//!
//! These compose the pieces `serve` composes, in process; the `serve` binary's own behaviour with
//! a taken or disabled port is tested in `crates/nullrouter-cli/tests/dashboard.rs`.

use std::net::{IpAddr, TcpListener, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use nullrouter_dashboard::spawn;
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::homes;
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::DashboardSettings;
use nullrouter_server::serve::{self, App};
use nullrouter_server::views;
use serde_json::json;
use tokio::sync::oneshot;

fn open(dir: &tempfile::TempDir) -> Arc<Engine> {
    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    Arc::new(engine)
}

fn settings(enabled: bool, listen: String) -> DashboardSettings {
    DashboardSettings { enabled, listen }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_taken_dashboard_port_is_reported_and_clients_are_still_served() {
    let dir = homes::empty();
    let engine = open(&dir);
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = taken.local_addr().unwrap().to_string();

    let (stop, stopped) = oneshot::channel::<()>();
    let dashboard = spawn(engine.clone(), &settings(true, listen.clone()), "test", async {
        let _ = stopped.await;
    })
    .await;
    assert!(dashboard.addr().is_none());
    let status = dashboard.status();
    assert!(status.enabled && !status.serving, "{status:?}");
    let error = status.error.as_deref().expect("the reason");
    assert!(error.starts_with(&format!("{listen}: ")) && error.contains("in use"), "{error}");
    assert_eq!(&engine.status.dashboard(), status, "server.status answers the same");

    // A client request is served as before: a keyless one is refused by 0router itself.
    let app = App::new(engine.clone()).unwrap();
    let client = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client_addr = client.local_addr().unwrap();
    let (client_stop, client_stopped) = oneshot::channel::<()>();
    let served = tokio::spawn(serve::run(app, client, async {
        let _ = client_stopped.await;
    }));
    let r = reqwest::Client::new()
        .post(format!("http://{client_addr}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}]}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert!(r.text().await.unwrap().contains("0router:"));

    // `dashboard status` and `check` read the same state.
    let home = OperatorHome::new(dir.path());
    let live = views::fetch_in_process(&engine, views::dashboard::NEEDS, &json!({})).await;
    let v = views::dashboard::build(&home, &json!({}), &live).unwrap().json;
    assert_eq!((&v["server"], &v["serving"]), (&json!("running"), &json!(false)));
    assert_eq!(v["error"], error);
    let live = views::fetch_in_process(&engine, views::check::NEEDS, &json!({})).await;
    let check = views::check::build(&home, &json!({}), &live).unwrap().json;
    let warned = check["notices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["text"] == format!("warning: dashboard not listening: {error}") && n["subject"] == "settings");
    assert!(warned, "{}", check["notices"]);

    let _ = (stop.send(()), client_stop.send(()));
    dashboard.stopped().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), served).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_dashboard_opens_no_port() {
    let dir = homes::empty();
    let engine = open(&dir);
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = free.local_addr().unwrap().to_string();
    drop(free);

    let dashboard = spawn(engine.clone(), &settings(false, listen.clone()), "test", async {}).await;
    assert!(dashboard.addr().is_none());
    let want = nullrouter_engine::status::DashboardStatus {
        enabled: false,
        listen: listen.clone(),
        serving: false,
        error: None,
    };
    assert_eq!(dashboard.status(), &want);
    assert_eq!(engine.status.dashboard(), want);
    assert!(tokio::net::TcpStream::connect(&listen).await.is_err(), "nothing listens on {listen}");
    dashboard.stopped().await;
}

/// An address of this host that isn't loopback, as the kernel would pick for an outbound packet
/// (a UDP `connect` sends nothing). `None` on a host with no route.
fn other_address() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_dashboard_binds_loopback_only() {
    let dir = homes::empty();
    let engine = open(&dir);
    let (stop, stopped) = oneshot::channel::<()>();
    let dashboard = spawn(engine, &settings(true, "127.0.0.1:0".into()), "test", async {
        let _ = stopped.await;
    })
    .await;
    let addr = dashboard.addr().expect("bound");
    assert!(addr.ip().is_loopback(), "{addr}");
    assert!(tokio::net::TcpStream::connect(addr).await.is_ok());

    match other_address() {
        None => eprintln!("skipped: this host has no non-loopback address to try"),
        Some(ip) => {
            let tried = tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect((ip, addr.port())))
                .await
                .expect("a refusal, not a hang");
            let e = tried.expect_err("the dashboard port must not answer on a non-loopback address");
            assert_eq!(e.kind(), std::io::ErrorKind::ConnectionRefused, "{ip}: {e}");
        }
    }
    let _ = stop.send(());
    dashboard.stopped().await;
}
