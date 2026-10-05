//! Token-expiry soak (spec 005 T049, SC-003): 2 s access tokens refreshed 1 s ahead (half
//! their lifetime caps the 30 min minimum lead), traffic in bursts with 5 s idle gaps over
//! more than 20 token lifetimes, through the server with the maintenance task running.
//! Zero failed requests, and no record carries an expiry class.
//!
//! An in-process client always runs. With `NR_HARNESS=1`, the Python and Node openai SDKs
//! (`tests/harness/{py,node}/soak.*`, see `tests/harness/README.md`) run alongside it.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use common::{chat_stream, chat_whole_text};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::maintenance;
use nullrouter_engine::records::{AttemptOutcome, ErrorClass, Outcome, Query};
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::mock_idp::{CLIENT_ID, DEVICE, TOKEN};
use nullrouter_engine::testkit::{MockIdp, MockUpstream, Step};
use nullrouter_engine::tokens::{self, Claims, TokenEntry};
use nullrouter_registry::{OperatorHome, SecretString};
use nullrouter_server::serve::{App, run};
use serde_json::{Value, json};

const LIFETIME: Duration = Duration::from_secs(2);
const SOAK: Duration = Duration::from_secs(46);
const GAP: Duration = Duration::from_secs(5);
/// The wait after each whole + streamed pair. Six 3 s bursts send at most 7,200 requests, so
/// every one keeps its record within `records::CAPACITY` however fast the machine is.
const PACE: Duration = Duration::from_millis(5);

struct Soak {
    _dir: tempfile::TempDir,
    engine: Arc<Engine>,
    mock: MockUpstream,
    idp: Arc<MockIdp>,
    base: String,
    key: String,
    stop: tokio_util::sync::CancellationToken,
}

/// A server with one sign-in account `soak/main` whose tokens come from the mock identity
/// provider, and a provider that accepts only tokens the identity provider holds valid.
async fn soak_server() -> Soak {
    let mock = MockUpstream::start().await;
    let idp = Arc::new(MockIdp::start().await);
    idp.set_token_lifetime(LIFETIME);
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::write(home.join("config.toml"), "allow_private_endpoints = true\n").unwrap();
    std::fs::create_dir(home.join("plugins")).unwrap();
    let plugin = format!(
        "schema = 2\nid = \"soak\"\ncategory = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[signin]\nflow = \"device_code\"\nclient_id = \"{CLIENT_ID}\"\ndevice_url = \"{}\"\ntoken_url = \"{}\"\nrefresh_lead = \"1s\"\n[[models]]\nid = \"m1\"\n",
        mock.url("/soak/chat/completions"),
        idp.url(DEVICE),
        idp.url(TOKEN)
    );
    std::fs::write(home.join("plugins/soak.toml"), plugin).unwrap();
    let accounts = "schema = 2\n[[account]]\nprovider = \"soak\"\nname = \"main\"\nkind = \"signin\"\n";
    nullrouter_engine::files::write_private(&home.join(nullrouter_engine::accounts::FILE), accounts).unwrap();
    let mut keys = Keys::default();
    let (key, _) = keys.issue("soak", None).unwrap();
    nullrouter_engine::files::write_private(&home.join(keys::FILE), &keys.to_toml()).unwrap();

    let (engine, report) = Engine::open_parity(OperatorHome::new(home)).unwrap();
    assert!(report.registry.unsupported.is_empty() && report.registry.skipped.is_empty(), "{report:#?}");
    let (access, refresh, expires_in) = idp.grant();
    let now = SystemTime::now();
    let entry = TokenEntry {
        provider: "soak".into(),
        name: "main".into(),
        access_token: SecretString::new(access),
        refresh_token: Some(SecretString::new(refresh)),
        expires_at: now + expires_in,
        scope: String::new(),
        claims: Claims::default(),
        hosts: engine.snapshot().registry.provider("soak").unwrap().token_hosts(),
        signed_in_at: now,
        last_refresh_at: None,
        state: None,
        state_since: None,
        state_reason: None,
    };
    tokens::update(home, "soak", "main", |slot| *slot = Some(entry)).unwrap();
    engine.reload_blocking().unwrap();

    let live = idp.clone();
    mock.respond(move |r| {
        let token = r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
        if !live.token_valid(token.trim_start_matches("Bearer ")) {
            return Step::json(401, json!({"error": {"message": "token expired", "type": "authentication_error"}}));
        }
        if r.json()["stream"] == true { chat_stream() } else { chat_whole_text("Hello") }
    });

    let engine = Arc::new(engine);
    let stop = tokio_util::sync::CancellationToken::new();
    let app = App::new(engine.clone()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let s = stop.clone();
    tokio::spawn(run(app, listener, async move { s.cancelled().await }));
    let s = stop.clone();
    maintenance::spawn(engine.clone(), async move { s.cancelled().await });
    Soak { _dir: dir, engine, mock, idp, base, key, stop }
}

/// Bursts of 3 s of requests (whole and streamed, [`PACE`] apart), then [`GAP`] idle, until
/// [`SOAK`] has passed. Returns `(sent, failed)`.
async fn in_process(base: &str, key: &str) -> (usize, Vec<String>) {
    let c = reqwest::Client::new();
    let url = format!("{base}/v1/chat/completions");
    let end = Instant::now() + SOAK;
    let (mut sent, mut failed) = (0, Vec::new());
    while Instant::now() < end {
        let burst = Instant::now() + Duration::from_secs(3);
        while Instant::now() < burst {
            for stream in [false, true] {
                let body =
                    json!({"model": "soak/m1", "stream": stream, "messages": [{"role": "user", "content": "hi"}]});
                sent += 1;
                let r = c
                    .post(&url)
                    .bearer_auth(key)
                    .header("content-type", "application/json")
                    .body(body.to_string())
                    .send()
                    .await;
                let ok = match r {
                    Ok(r) if r.status().is_success() => {
                        let text = r.text().await.unwrap_or_default();
                        if stream {
                            text.contains("\"Hel\"") && text.contains("[DONE]") && !text.contains("\"error\"")
                        } else {
                            serde_json::from_str::<Value>(&text)
                                .is_ok_and(|v| v["choices"][0]["message"]["content"] == "Hello")
                        }
                    }
                    _ => false,
                };
                if !ok {
                    failed.push(format!("request {sent} (stream {stream})"));
                }
            }
            tokio::time::sleep(PACE).await;
        }
        tokio::time::sleep(GAP).await;
    }
    (sent, failed)
}

/// One SDK soak script; `None` when its SDK isn't installed.
fn sdk(cmd: &str, script: &Path, s: &Soak) -> Option<std::process::Output> {
    let harness = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/harness");
    let program = match cmd {
        "python" => harness.join(".venv/bin/python"),
        _ => "node".into(),
    };
    if cmd == "python" && !program.exists() || cmd == "node" && !harness.join("node_modules/openai").exists() {
        return None;
    }
    let out = std::process::Command::new(program)
        .arg(script)
        .current_dir(&harness)
        .env("NR_BASE", &s.base)
        .env("NR_KEY", &s.key)
        .env("NR_MODEL", "soak/m1")
        .env("NR_SOAK_SECS", SOAK.as_secs().to_string())
        .env("NR_SOAK_GAP", GAP.as_secs().to_string())
        .output()
        .unwrap();
    Some(out)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tokens_stay_fresh_across_twenty_lifetimes_with_idle_gaps() {
    let s = Arc::new(soak_server().await);
    let harness = std::env::var("NR_HARNESS").as_deref() == Ok("1");
    let sdk_runs = AtomicUsize::new(0);
    let mut scripts = Vec::new();
    if harness {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/harness");
        for (cmd, script) in [("python", dir.join("py/soak.py")), ("node", dir.join("node/soak.mjs"))] {
            let s = s.clone();
            scripts.push((cmd, tokio::task::spawn_blocking(move || sdk(cmd, &script, &s))));
        }
    } else {
        eprintln!("SDK soak skipped: set NR_HARNESS=1 to add the Python and Node SDK clients");
    }
    let (sent, failed) = in_process(&s.base, &s.key).await;
    assert!(failed.is_empty(), "{} of {sent} in-process requests failed: {failed:?}", failed.len());
    for (cmd, task) in scripts {
        match task.await.unwrap() {
            None => panic!("NR_HARNESS=1 but the {cmd} SDK isn't installed (tests/harness/README.md)"),
            Some(out) => {
                let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
                println!("{cmd}: {text}");
                assert!(out.status.success(), "{cmd} soak failed: {text}");
                sdk_runs.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    s.stop.cancel();

    // ≥ 20 lifetimes passed, each covered by a refresh.
    let refreshes = s.idp.refresh_calls();
    assert!(refreshes >= 20, "{refreshes} refreshes over {} s", SOAK.as_secs());
    let records = s.engine.records.query(&Query::default());
    assert!(records.len() >= sent, "{} records for {sent} requests", records.len());
    let expiry = [ErrorClass::Auth, ErrorClass::TokenRefreshing, ErrorClass::NeedsSignIn];
    for r in &records {
        assert_eq!(r.outcome, Outcome::Succeeded, "{r:#?}");
        for a in &r.attempts {
            let class = match &a.outcome {
                Some(AttemptOutcome::Failed { class, .. }) => Some(*class),
                Some(AttemptOutcome::Skipped { class, .. }) => *class,
                _ => None,
            };
            assert!(!class.is_some_and(|c| expiry.contains(&c)), "an expiry-class attempt: {r:#?}");
        }
    }
    assert!(s.mock.received().len() >= sent);
    println!("soak: {sent} in-process requests, {} SDK runs, {refreshes} refreshes", sdk_runs.load(Ordering::SeqCst));
}
