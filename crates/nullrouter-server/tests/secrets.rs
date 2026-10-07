//! No secret leaves where it belongs (T102, SC-006, US5-5, FR-033). The provider echoes
//! the account's secret in its error bodies and headers; neither that secret nor the agent
//! key may show up in the logs, any record, any client response, the operator socket's
//! answers (what the CLI prints), the headers the provider got beyond its auth header, or
//! the plugin-visible registry.
//!
//! Sign-in secrets (spec 005 T091, SC-009): access and refresh tokens, device codes,
//! authorization codes and PKCE verifiers from a mock identity provider, and the token the
//! anthropic usage poll carries, through a full flow: sign-in with the CLI's driver (code
//! page paste-back, device code, a denied device code), requests (served, failed with the
//! token quoted, an informational error), refresh with rotation and a permanent failure, a
//! rejected-then-refused token, quota polls (one failing with the token quoted). Then the
//! scan: logs, records, client responses, CLI output (sign-in screens, `quota` text, the
//! `accounts list` token column), operator socket answers, `quota/*.jsonl` and the
//! plugin-visible registry. Only the `…last4` form may appear.
//!
//! Routing (spec 006 T092, SC-012): warm, cold and overflow traffic carrying a sentinel prompt,
//! then the scan adds the record and routing journals on disk, the routing view (text as the CLI
//! renders it, and JSON), every record's JSON (what `records show` prints, text or `--json`, is
//! drawn from its fields) and `routing.health`: no secret and no prompt text anywhere.

mod common;

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use axum::body::Bytes;
use common::{SECRET, bundled_at_mock, chat_stream, reply_by_wire, server};
use nullrouter_cli::quota_text;
use nullrouter_cli::signin::{PASTE_PROMPT, SignIn};
use nullrouter_engine::accounts;
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::quota::poll::PollTiming;
use nullrouter_engine::records::Query;
use nullrouter_engine::signin::SignInHttp;
use nullrouter_engine::signin::refresh::Refreshed;
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::mock_idp::{AUTHORIZE, CLIENT_ID, DEVICE, TOKEN};
use nullrouter_engine::testkit::{DevicePoll, Failure, MockIdp, MockUpstream, QuotaRoute, Received, Step};
use nullrouter_engine::tokens::TokenStore;
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator;
use nullrouter_server::serve::{App, run};
use serde_json::json;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An error that quotes the credential the request carried, in the body and a header.
fn echo(status: u16, r: &Received) -> Step {
    let auth = r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    Step::Reply {
        status,
        headers: vec![("content-type".into(), "application/json".into()), ("x-debug-auth".into(), auth.clone())],
        body: Bytes::from(
            json!({"error": {"message": format!("invalid key {auth}"), "type": "invalid_request_error"}}).to_string(),
        ),
    }
}

fn reply(r: &Received) -> Step {
    let said = r.json()["messages"][0]["content"].as_str().unwrap_or_default().to_owned();
    match said.as_str() {
        "fail400" => echo(400, r),
        "fail401" => echo(401, r),
        "fail500" => echo(500, r),
        _ => chat_stream(),
    }
}

/// Every test's tracing output, at TRACE, in one buffer (the subscriber is global).
fn logs() -> Captured {
    static LOGS: OnceLock<Captured> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = Captured::default();
        let sink = logs.clone();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .init();
        logs
    })
    .clone()
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// Where in `places` any of `sentinels` shows, with the text before it.
fn leaks_of(places: &[(&str, String)], sentinels: &[&str]) -> Vec<String> {
    let mut leaks = Vec::new();
    for (place, text) in places {
        for secret in sentinels {
            if let Some(at) = text.find(secret) {
                let from = text[..at].char_indices().rev().nth(80).map_or(0, |(i, _)| i);
                leaks.push(format!("{place}: …{}…", &text[from..(at + secret.len()).min(text.len())]));
            }
        }
    }
    leaks
}

#[tokio::test]
async fn secrets_appear_nowhere_but_the_upstream_auth_header() {
    let logs = logs();

    let s = server().await;
    s.mock.respond(reply);
    let key = s.key.clone();
    let sentinels = [SECRET, key.as_str()];
    let c = reqwest::Client::new();
    let mut seen = Vec::new();
    for (door, said, stream) in [
        ("chat", "fail400", false),
        ("chat", "fail401", true),
        ("chat", "fail500", true),
        ("messages", "fail500", false),
        ("messages", "ok", true),
        ("chat", "ok", true),
    ] {
        let req = match door {
            "chat" => c.post(format!("{}/v1/chat/completions", s.base)).bearer_auth(&s.key),
            _ => c
                .post(format!("{}/v1/messages", s.base))
                .header("x-api-key", &s.key)
                .header("anthropic-version", "2023-06-01"),
        };
        let body = json!({"model": "mockco/m1", "max_tokens": 64, "stream": stream, "messages": [{"role": "user", "content": said}]});
        let r = req.body(body.to_string()).send().await.unwrap();
        let headers = format!("{:?}", r.headers());
        seen.push(format!("{door} {said}: {} {headers}\n{}", r.status(), r.text().await.unwrap()));
    }
    // A wrong key's refusal must not quote it either.
    let r = c
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(format!("{}x", s.key))
        .body("{}")
        .send()
        .await
        .unwrap();
    let headers = format!("{:?}", r.headers());
    seen.push(format!("wrong key: {headers}\n{}", r.text().await.unwrap()));

    let records = serde_json::to_string(&s.engine.records.query(&Query::default())).unwrap();
    assert!(records.contains("invalid key"), "the provider's errors reached the records: {records}");
    let mut socket = Vec::new();
    for op in [json!({"op": "records.list"}), json!({"op": "accounts.state"}), json!({"op": "reload"})] {
        socket.push(operator::handle(&s.engine, &op).await.to_string());
    }
    for r in s.engine.records.query(&Query::default()) {
        socket.push(operator::handle(&s.engine, &json!({"op": "records.get", "id": r.id})).await.to_string());
    }
    let upstream: Vec<String> = s
        .mock
        .received()
        .iter()
        .map(|r| {
            let beyond_auth: Vec<String> =
                r.headers.iter().filter(|(k, _)| *k != "authorization").map(|(k, v)| format!("{k}: {v:?}")).collect();
            format!("{} {}\n{}", r.path_and_query, beyond_auth.join("\n"), String::from_utf8_lossy(&r.body))
        })
        .collect();
    let registry = format!("{:?}", s.engine.snapshot().registry);
    let carried = s.mock.received().iter().all(|r| r.headers["authorization"] == format!("Bearer {SECRET}"));
    assert!(carried, "the provider gets the secret in its auth header");
    drop(s);
    let logs = logs.text();
    assert!(!logs.is_empty(), "no logs were captured");

    let places: [(&str, String); 6] = [
        ("logs", logs),
        ("records", records),
        ("client responses", seen.join("\n")),
        ("operator socket answers", socket.join("\n")),
        ("upstream requests beyond the auth header", upstream.join("\n")),
        ("registry", registry),
    ];
    let leaks = leaks_of(&places, &sentinels);
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

// ---- sign-in secrets (spec 005 T091, SC-009, FR-033) ------------------------------------

/// What the mock identity provider's secrets all carry: access and refresh tokens
/// (`mock-access-SENTINEL-n`, `mock-refresh-SENTINEL-n`), device codes and authorization
/// codes. PKCE verifiers are random; they're read back from the token requests.
const IDP_SENTINEL: &str = "SENTINEL";

/// `bundled_at_mock` with the sign-in endpoints at `idp` and its client id.
fn signin_plugin(mock: &MockUpstream, idp: &MockIdp, id: &str) -> String {
    let base = mock.url("");
    let text = bundled_at_mock(mock, id)
        .replace(&format!("{base}/oauth/authorize"), &idp.url(AUTHORIZE))
        .replace(&format!("{base}/v1/oauth/token"), &idp.url(TOKEN))
        .replace(&format!("{base}/oauth2/device/code"), &idp.url(DEVICE))
        .replace(&format!("{base}/oauth2/token"), &idp.url(TOKEN));
    text.lines()
        .map(|l| if l.starts_with("client_id = ") { format!("client_id = \"{CLIENT_ID}\"") } else { l.to_owned() })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What the CLI's sign-in driver printed, readable while it runs.
#[derive(Clone, Default)]
struct Screen(Arc<Mutex<Vec<u8>>>);

impl Write for Screen {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Screen {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    /// Waits for the printed authorize link and the paste prompt after it.
    async fn link(&self) -> String {
        for _ in 0..500 {
            let t = self.text();
            if t.ends_with(PASTE_PROMPT)
                && let Some(l) = t.lines().find(|l| l.starts_with("  http"))
            {
                return l.trim().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no link printed: {}", self.text());
    }
}

/// One `accounts signin` through the CLI's driver; `script` plays the operator and the
/// browser. Returns everything the CLI would print (prompts, then the result line).
async fn cli_signin<F: Future<Output = ()>>(
    engine: &Engine,
    provider: &str,
    name: &str,
    script: impl FnOnce(Screen, UnboundedSender<String>) -> F,
) -> (bool, String) {
    let st = engine.snapshot();
    let p = st.registry.provider(provider).unwrap();
    let http = SignInHttp::new(true).with_timeout(Duration::from_secs(5));
    let job = SignIn { home: engine.home(), provider: p, name, http: &http, accept_terms_risk: true, browser: None };
    let (tx, mut rx) = unbounded_channel();
    let screen = Screen::default();
    let mut out = screen.clone();
    let cancel = CancellationToken::new();
    let (r, ()) = tokio::join!(job.run(&mut rx, &mut out, &cancel), script(screen.clone(), tx));
    let ok = r.is_ok();
    let last = match r {
        Ok(done) => done.line(),
        Err(ended) => ended.message,
    };
    (ok, format!("{}{last}\n", screen.text()))
}

/// The provider answers: quota routes with 9router-shaped samples (the anthropic usage
/// route echoes the token it got while `usage_fails`), and inference "Hello" in every wire,
/// or an error quoting the token when the message says `fail401`/`fail500`.
fn signin_reply(usage_fails: Arc<AtomicBool>) -> impl Fn(&Received) -> Step + Send + Sync + 'static {
    move |r| {
        let path = r.path_and_query.split('?').next().unwrap_or_default().to_owned();
        if path == QuotaRoute::AnthropicUsage.path() && usage_fails.load(Ordering::SeqCst) {
            return echo(500, r);
        }
        if let Some(route) = QuotaRoute::ALL.into_iter().find(|q| q.path() == path) {
            return route.sample();
        }
        let body = String::from_utf8_lossy(&r.body);
        if body.contains("fail401") {
            echo(401, r)
        } else if body.contains("fail500") {
            echo(500, r)
        } else {
            reply_by_wire(r)
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sign_in_secrets_appear_nowhere() {
    let logs = logs();
    let mock = MockUpstream::start().await;
    let idp = MockIdp::start().await;
    idp.set_rotation(true);
    idp.set_device_timing(1, 900);
    idp.set_id_claims(Some(json!({ "email": "work@example.com", "sub": "s-1" })));
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::write(
        home.join("config.toml"),
        "allow_private_endpoints = true\n[plugin_decisions]\nanthropic = \"replace\"\n\"grok-cli\" = \"replace\"\n",
    )
    .unwrap();
    std::fs::create_dir(home.join("plugins")).unwrap();
    for id in ["anthropic", "grok-cli"] {
        std::fs::write(home.join(format!("plugins/{id}.toml")), signin_plugin(&mock, &idp, id)).unwrap();
    }
    nullrouter_engine::files::write_private(&home.join(accounts::FILE), "schema = 2\n").unwrap();
    let mut keys = Keys::default();
    let (key, _) = keys.issue("laptop", None).unwrap();
    nullrouter_engine::files::write_private(&home.join(keys::FILE), &keys.to_toml()).unwrap();
    let usage_fails = Arc::new(AtomicBool::new(false));
    mock.respond(signin_reply(usage_fails.clone()));

    let (engine, report) = Engine::open_parity(OperatorHome::new(home)).unwrap();
    let r = &report.registry;
    assert!(r.unsupported.is_empty() && r.skipped.is_empty() && r.pending_conflicts.is_empty(), "{report:#?}");
    let engine = Arc::new(engine);
    engine.quota.set_timing(PollTiming {
        scale: 1.0,
        retry_after: Duration::from_millis(50),
        jitter: 0.0,
        timeout: Duration::from_secs(5),
    });
    let stop = CancellationToken::new();
    let app = App::new(engine.clone()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let s = stop.clone();
    tokio::spawn(run(app, listener, async move { s.cancelled().await }));
    let socket = operator::bind(engine.home()).unwrap();
    let s = stop.clone();
    tokio::spawn(operator::serve(engine.clone(), socket, async move { s.cancelled().await }));

    // Sign-in through the CLI's driver: anthropic by code page paste-back (PKCE), grok-cli
    // by device code (pending once), and a grok-cli sign-in the operator denies.
    let mut cli = Vec::new();
    let browser = &idp;
    let (ok, printed) =
        cli_signin(&engine, "anthropic", "max", |screen: Screen, tx: UnboundedSender<String>| async move {
            let link = screen.link().await;
            let location = browser.browse(&link).await;
            let q = |k: &str| {
                url::Url::parse(&location).unwrap().query_pairs().find(|(n, _)| n == k).unwrap().1.into_owned()
            };
            tx.send(format!("{}#{}", q("code"), q("state"))).unwrap();
        })
        .await;
    assert!(ok, "{printed}");
    cli.push(printed);
    idp.push_device([DevicePoll::Pending]);
    let (ok, printed) = cli_signin(&engine, "grok-cli", "work", |_, _| async {}).await;
    assert!(ok && printed.contains("; applied"), "{printed}");
    cli.push(printed);
    idp.push_device([DevicePoll::Denied]);
    let (ok, printed) = cli_signin(&engine, "grok-cli", "denied", |_, _| async {}).await;
    assert!(!ok, "{printed}");
    cli.push(printed);
    let st = engine.snapshot();
    assert!(st.accounts.get("anthropic", "max").is_some() && st.accounts.get("grok-cli", "work").is_some());

    // Requests: served, a provider error quoting the token, and (below) an informational
    // error for an account that needs signing in again.
    let c = reqwest::Client::new();
    let seen = std::cell::RefCell::new(Vec::new());
    let send = |model: &'static str, said: &'static str, stream: bool| {
        let body = json!({"model": model, "max_tokens": 64, "stream": stream, "messages": [{"role": "user", "content": said}]});
        c.post(format!("{base}/v1/messages"))
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
            .body(body.to_string())
            .send()
    };
    let ask = async |model: &'static str, said: &'static str, stream: bool| {
        let r = send(model, said, stream).await.unwrap();
        let status = r.status();
        let headers = format!("{:?}", r.headers());
        let body = r.text().await.unwrap();
        seen.borrow_mut().push(format!("{model} {said} stream {stream}: {status} {headers}\n{body}"));
        status
    };
    assert_eq!(ask("anthropic/claude-sonnet-4-20250514", "ok", false).await, 200);
    assert_eq!(ask("anthropic/claude-sonnet-4-20250514", "ok", true).await, 200);
    assert_eq!(ask("grok-cli/grok-4.5", "ok", false).await, 200);
    assert_eq!(ask("grok-cli/grok-4.5", "ok", true).await, 200);
    assert!(!ask("grok-cli/grok-4.5", "fail500", false).await.is_success());

    // Quota: a poll, then a failed one whose answer quotes the token.
    let good = engine.poll_quota("anthropic", "max").await.unwrap();
    assert!(good.ok(), "{good:?}");
    assert!(engine.poll_quota("grok-cli", "work").await.is_some());
    usage_fails.store(true, Ordering::SeqCst);
    let mut socket_answers = Vec::new();
    let failed = operator::handle(&engine, &json!({"op": "quota.poll", "provider": "anthropic", "name": "max"})).await;
    assert!(failed.to_string().contains("500"), "the poll failed: {failed}");
    socket_answers.push(failed.to_string());
    usage_fails.store(false, Ordering::SeqCst);

    // Refresh with rotation, then a permanent failure: grok-cli/work needs signing in.
    let refresh_token = || {
        let store = TokenStore::load(home).unwrap();
        store.get("grok-cli", "work").unwrap().refresh_token.as_ref().unwrap().with_exposed(str::to_owned)
    };
    let before = refresh_token();
    assert!(matches!(engine.refresh_account("grok-cli", "work").await, Refreshed::Fresh));
    assert_ne!(refresh_token(), before, "the refresh token rotated");
    idp.fail_token([Failure::permanent("invalid_grant")]);
    assert!(matches!(engine.refresh_account("grok-cli", "work").await, Refreshed::Permanent(_)));
    assert!(!ask("grok-cli/grok-4.5", "ok", false).await.is_success(), "the informational error");
    let last = seen.borrow().last().cloned().unwrap();
    assert!(last.contains("needs sign-in"), "{last}");
    // A token the provider rejects, refreshed (rotated) and rejected again: refused.
    assert!(!ask("anthropic/claude-sonnet-4-20250514", "fail401", false).await.is_success());
    assert!(idp.refresh_calls() >= 3, "{} refreshes", idp.refresh_calls());

    // What the operator and the CLI see.
    for op in [
        json!({"op": "accounts.state"}),
        json!({"op": "quota.list"}),
        json!({"op": "records.list"}),
        json!({"op": "reload"}),
    ] {
        socket_answers.push(operator::handle(&engine, &op).await.to_string());
    }
    for r in engine.records.query(&Query::default()) {
        socket_answers.push(operator::handle(&engine, &json!({"op": "records.get", "id": r.id})).await.to_string());
    }
    // Over the socket itself, as the CLI asks.
    let h = engine.home().clone();
    let (state, quota) = tokio::task::spawn_blocking(move || {
        (
            operator::call(&h, &json!({"op": "accounts.state"})).unwrap(),
            operator::call(&h, &json!({"op": "quota.list"})).unwrap(),
        )
    })
    .await
    .unwrap();
    socket_answers.push(state.to_string());
    socket_answers.push(quota.to_string());
    // `nullrouter quota` prints this text; `accounts list` shows the stored token's last four.
    cli.push(quota_text::render(quota["accounts"].as_array().unwrap(), SystemTime::now(), 0));
    let store = TokenStore::load(home).unwrap();
    for e in &store.entries {
        cli.push(format!("{}/{} {} {:?}", e.provider, e.name, e.shown_token(), e.state_reason));
    }
    let states: Vec<&str> =
        state["accounts"].as_array().unwrap().iter().map(|a| a["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["refused", "needs_sign_in"], "{state}");

    engine.history.drain();
    let mut history = String::new();
    let mut dirs = vec![home.join("quota")];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            if e.path().is_dir() {
                dirs.push(e.path());
            } else {
                history += &format!("{}\n{}\n", e.path().display(), std::fs::read_to_string(e.path()).unwrap());
            }
        }
    }
    assert!(history.contains(".jsonl") && history.contains("500"), "the failed poll is kept: {history}");

    let records = serde_json::to_string(&engine.records.query(&Query::default())).unwrap();
    assert!(records.contains("invalid key"), "the provider's errors reached the records: {records}");
    let registry = format!("{:?}", engine.snapshot().registry);
    // The provider got the tokens, so the sentinels are live: access tokens on requests
    // and quota polls, codes and verifiers at the identity provider.
    let got = mock.received();
    let usage = got.iter().filter(|r| r.path_and_query == QuotaRoute::AnthropicUsage.path());
    assert!(usage.clone().count() >= 2 && usage.clone().all(|r| bearer(r).contains(IDP_SENTINEL)), "usage token");
    let verifiers: Vec<String> =
        idp.token_requests().iter().filter_map(|t| t.get("code_verifier").map(str::to_owned)).collect();
    assert!(!verifiers.is_empty(), "a PKCE exchange ran");
    assert!(idp.token_requests().iter().any(|t| t.get("device_code").is_some_and(|c| c.contains(IDP_SENTINEL))));
    assert!(idp.token_requests().iter().any(|t| t.get("code").is_some_and(|c| c.contains(IDP_SENTINEL))));
    stop.cancel();
    drop(engine);
    let logs = logs.text();
    assert!(logs.contains("anthropic") || logs.contains("grok-cli"), "the flow's logs were captured");

    let mut sentinels: Vec<&str> = vec![IDP_SENTINEL, key.as_str()];
    sentinels.extend(verifiers.iter().map(String::as_str));
    let places: [(&str, String); 7] = [
        ("logs", logs),
        ("records", records),
        ("client responses and informational errors", seen.borrow().join("\n")),
        ("CLI output", cli.join("\n")),
        ("operator socket answers", socket_answers.join("\n")),
        ("quota history", history),
        ("plugin-visible registry", registry),
    ];
    let leaks = leaks_of(&places, &sentinels);
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

fn bearer(r: &Received) -> String {
    r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned()
}

// ---- routing (spec 006 T092, SC-012) -----------------------------------------------------

const PROMPT: &str = "PROMPT-SENTINEL-T092 the operator must never read this";
const SUB_A: &str = "sk-sub-a-SENTINEL-T092";
const SUB_B: &str = "sk-sub-b-SENTINEL-T092";
const PAYG: &str = "sk-pay-SENTINEL-T092";

const METERED: &str = "\n[routing.cache]\nmode = \"automatic\"\nlifetime = \"5m\"\nmin_tokens = 0\n\n[[routing.window]]\nname = \"5h\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\ncapacity = 1000000\nreserve = \"10%\"\n";
const PRICED: &str =
    "\n[routing.cache]\nmode = \"automatic\"\nmin_tokens = 0\n\n[[routing.price]]\ninput = 3.0\noutput = 12.0\n";

fn routed_plugin(mock: &MockUpstream, id: &str, extra: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n{extra}",
        mock.url(&format!("/{id}/chat/completions"))
    )
}

/// Every file under `dir`, as text, named by its path.
fn files_under(dir: &std::path::Path) -> Vec<(String, String)> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in read.flatten() {
        let path = e.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push((path.display().to_string(), String::from_utf8_lossy(&bytes).into_owned()));
        }
    }
    out
}

#[tokio::test]
async fn routing_keeps_no_secret_and_no_prompt() {
    use nullrouter_engine::testkit::{MockQuota, SimQuota, SimWindow};
    let logs = logs();
    let quota_mock = MockQuota::start().await;
    let quota = SimQuota::new();
    quota_mock.simulate(&quota);
    quota.account(SUB_A, "alpha/a");
    quota.account(SUB_B, "alpha/b");
    let reset = nullrouter_engine::clock::rfc3339(SystemTime::now() + Duration::from_secs(3600));
    let left = |used: f64| vec![SimWindow::new("5h", "tokens", 1_000_000.0, &reset).used(used)];
    quota.set("alpha/a", left(100_000.0));
    quota.set("alpha/b", left(300_000.0));
    let url = quota_mock.url(QuotaRoute::Sim);
    let accounts = format!(
        "schema = 2\n[[account]]\nprovider = \"alpha\"\nname = \"a\"\nsecret = \"{SUB_A}\"\n\
         [[account]]\nprovider = \"alpha\"\nname = \"b\"\nsecret = \"{SUB_B}\"\n\
         [[account]]\nprovider = \"pay\"\nname = \"key\"\nsecret = \"{PAYG}\"\n"
    );
    let s = common::server_custom(
        |m| {
            let alpha = format!("{}\n{}", routed_plugin(m, "alpha", METERED), SimQuota::quota_toml(&url, &[("5h", "tokens")]));
            vec![("alpha", alpha), ("pay", routed_plugin(m, "pay", PRICED))]
        },
        &accounts,
        "[[unified_model]]\nname = \"u\"\nmembers = [{ provider = \"alpha\", model = \"m1\" }, { provider = \"pay\", model = \"m1\" }]\n",
    )
    .await;
    s.mock.respond(|_| chat_stream());
    s.engine.poll_quota("alpha", "a").await.expect("polled");
    s.engine.poll_quota("alpha", "b").await.expect("polled");

    let c = reqwest::Client::new();
    let send = |messages: serde_json::Value| {
        c.post(format!("{}/v1/chat/completions", s.base))
            .bearer_auth(&s.key)
            .body(json!({"model": "u", "stream": true, "messages": messages}).to_string())
            .send()
    };
    let first = json!([{"role": "system", "content": PROMPT}, {"role": "user", "content": PROMPT}]);
    let follow = json!([
        {"role": "system", "content": PROMPT}, {"role": "user", "content": PROMPT},
        {"role": "assistant", "content": "hi"}, {"role": "user", "content": format!("{PROMPT} again")},
    ]);
    let mut seen = Vec::new();
    for body in [first.clone(), follow, json!([{"role": "user", "content": format!("{PROMPT} cold")}])] {
        let r = send(body).await.unwrap();
        let head = format!("{} {:?}", r.status(), r.headers());
        seen.push(format!("{head}\n{}", r.text().await.unwrap()));
    }
    // Both subscriptions at their floor: the next cold request overflows to pay-as-you-go.
    quota.set("alpha/a", left(995_000.0));
    quota.set("alpha/b", left(995_000.0));
    s.engine.poll_quota("alpha", "a").await.expect("polled");
    s.engine.poll_quota("alpha", "b").await.expect("polled");
    let r = send(json!([{"role": "user", "content": format!("{PROMPT} overflow")}])).await.unwrap();
    let head = format!("{} {:?}", r.status(), r.headers());
    seen.push(format!("{head}\n{}", r.text().await.unwrap()));

    let mut records = Vec::new();
    for r in s.engine.records.query(&Query::default()) {
        records.push(operator::handle(&s.engine, &json!({"op": "records.get", "id": r.id})).await);
    }
    let kinds: Vec<&str> = records.iter().filter_map(|r| r["record"]["decision"]["kind"].as_str()).collect();
    for kind in ["warm", "cold", "overflow"] {
        assert!(kinds.contains(&kind), "no {kind} decision among {kinds:?}");
    }
    let view = operator::handle(&s.engine, &json!({"op": "routing.view"})).await;
    assert_eq!(view["ok"], true, "{view:#}");
    let mut socket = vec![view.to_string(), nullrouter_cli::routing_text::render(&view, SystemTime::now())];
    for op in ["records.list", "routing.health", "accounts.state"] {
        socket.push(operator::handle(&s.engine, &json!({"op": op})).await.to_string());
    }
    socket.extend(records.iter().map(ToString::to_string));
    let registry = format!("{:?}", s.engine.snapshot().registry);
    let journal = s.engine.journal.clone();
    tokio::task::spawn_blocking(move || journal.flush_blocking()).await.unwrap();
    let mut on_disk = files_under(&s.home().join("records"));
    on_disk.extend(files_under(&s.home().join("routing")).into_iter().filter(|(p, _)| !p.ends_with("salt")));
    assert!(
        on_disk.iter().any(|(p, _)| p.ends_with(".jsonl") && p.contains("records")),
        "no record journal: {on_disk:?}"
    );
    let key = s.key.clone();
    drop(s);

    let mut places: Vec<(&str, String)> = vec![
        ("logs", logs.text()),
        ("client responses", seen.join("\n")),
        ("operator socket answers and the routing view", socket.join("\n")),
        ("registry", registry),
    ];
    let disk = on_disk.iter().map(|(p, t)| format!("{p}\n{t}")).collect::<Vec<_>>().join("\n");
    places.push(("records/ and routing/", disk));
    let leaks = leaks_of(&places, &[SUB_A, SUB_B, PAYG, key.as_str(), "PROMPT-SENTINEL-T092"]);
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

/// Spec 008 (FR-007, SC-006, research R9): every view, by both routes, on a home whose files
/// hold secret values: no `json` or `extra` carries more of one than its last four characters.
#[tokio::test(flavor = "multi_thread")]
async fn views_show_no_secret_beyond_its_last_four() {
    use nullrouter_engine::testkit::homes;
    use nullrouter_server::views;

    // The values `homes::full` plants in accounts.toml and tokens.toml.
    let planted = [
        "sk-fixture-main-AAAA0001",
        "sk-fixture-spare-BBBB0002",
        "xai-access-fixture-WORK1111",
        "xai-refresh-fixture-WORK2222",
        "xai-access-fixture-OLD33333",
        "xai-access-fixture-GONE4444",
    ];
    let dir = homes::full();
    let home = OperatorHome::new(dir.path());
    let (engine, _) = Engine::open(home.clone()).unwrap();
    let engine = Arc::new(engine);
    let listener = operator::bind(&home).unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(operator::serve(engine.clone(), listener, async move {
        let _ = stopped.await;
    }));
    let mut scanned = 0;
    for (name, needs, args, build) in common::view_cases() {
        let by_socket = {
            let (home, args) = (home.clone(), args.clone());
            tokio::task::spawn_blocking(move || build(&home, &args, &views::fetch_socket(&home, needs, &args)))
                .await
                .unwrap()
        };
        let by_server = {
            let (home, args2) = (home.clone(), args.clone());
            views::run_in_process(&engine, needs, &args, move |live| build(&home, &args2, live)).await
        };
        for view in [by_socket, by_server].into_iter().flatten() {
            let text = format!("{}{}", view.json, view.extra);
            for secret in planted {
                // The last four characters are what listings show; anything longer is a leak.
                let beyond = &secret[secret.len() - 5..];
                assert!(!text.contains(beyond), "{name}: a view shows more of {secret:?} than its last four");
            }
            scanned += 1;
        }
    }
    assert!(scanned > 30, "the scan ran over {scanned} answers");
    stop.send(()).unwrap();
    task.await.unwrap();
}

// ---- model tests (spec 011 T051, SC-008) -------------------------------------------------

const TEST_KEY: &str = "sk-test-SENTINEL-T051";
const OUTPUT: &str = "OUTPUT-SENTINEL-T051 the model's answer is never kept";

/// A pair test refused with the key quoted, then a combo test answered with a sentinel output:
/// neither the key, the test prompt nor the output shows in the logs, the test's records, what
/// `test.plan`, `test.run` and `verdicts.list` answer, or `routing/verdicts.jsonl`.
#[tokio::test]
async fn model_tests_keep_no_secret_prompt_or_output() {
    use std::sync::atomic::AtomicUsize;

    use nullrouter_engine::tests as model_tests;
    use nullrouter_engine::verdict::{Source, State};

    let logs = logs();
    let accounts = format!("schema = 2\n[[account]]\nprovider = \"alpha\"\nname = \"a\"\nsecret = \"{TEST_KEY}\"\n");
    let s = common::server_custom(
        |m| vec![("alpha", routed_plugin(m, "alpha", ""))],
        &accounts,
        "[[unified_model]]\nname = \"u\"\nmembers = [{ provider = \"alpha\", model = \"m1\" }]\n\
         [[combo]]\nname = \"c\"\nmembers = [\"u\"]\n",
    )
    .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let n = calls.clone();
    s.mock.respond(move |r| match n.fetch_add(1, Ordering::SeqCst) {
        0 => {
            let auth = r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
            let message = format!("The model m1 does not exist for {auth}");
            Step::Reply {
                status: 404,
                headers: vec![("content-type".into(), "application/json".into()), ("x-debug-auth".into(), auth)],
                body: Bytes::from(json!({"error": {"message": message}}).to_string()),
            }
        }
        _ => Step::json(
            200,
            json!({"id": "x", "object": "chat.completion", "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": OUTPUT}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 9}}),
        ),
    });

    let mut socket = Vec::new();
    for target in ["alpha/m1", "c"] {
        socket.push(operator::handle(&s.engine, &json!({"op": "test.plan", "target": target})).await.to_string());
    }
    let st = s.engine.snapshot();
    let planned = model_tests::expand(&s.engine, &st, Some("alpha/m1"), None).unwrap();
    let stop = CancellationToken::new();
    let pair = model_tests::run_pair(&s.engine, &planned[0], Source::Test, "tr_s", &stop).await.unwrap();
    assert_eq!(pair.state, Some(State::Broken), "{pair:?}");
    let combo = model_tests::combo::run_combo(&s.engine, "c", Source::Test, "tr_s", &stop).await.unwrap().unwrap();
    assert_eq!(combo.state, State::Pass, "{combo:?}");
    // What `test.run` streams.
    socket.push(json!({"event": "result", "result": pair}).to_string());
    socket.push(json!({"event": "combo", "result": combo}).to_string());
    let list = operator::handle(&s.engine, &json!({"op": "verdicts.list"})).await;
    assert_eq!(list["combos"][0]["combo"], "c", "{list}");
    socket.push(list.to_string());

    let mut records = Vec::new();
    for r in s.engine.records.query(&Query::default()) {
        assert!(r.test.is_some(), "every call here was a test: {r:?}");
        records.push(operator::handle(&s.engine, &json!({"op": "records.get", "id": r.id})).await.to_string());
    }
    assert_eq!(records.len(), 2);
    let journal = s.engine.journal.clone();
    tokio::task::spawn_blocking(move || journal.flush_blocking()).await.unwrap();
    let verdicts = std::fs::read_to_string(s.home().join("routing/verdicts.jsonl")).unwrap();
    assert!(verdicts.contains("\"combo\":\"c\""), "{verdicts}");
    let mut on_disk = files_under(&s.home().join("records"));
    on_disk.push(("routing/verdicts.jsonl".into(), verdicts));
    let key = s.key.clone();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    drop(s);

    let disk = on_disk.iter().map(|(p, t)| format!("{p}\n{t}")).collect::<Vec<_>>().join("\n");
    let places: Vec<(&str, String)> = vec![
        ("logs", logs.text()),
        ("test.plan, test.run and verdicts.list answers", socket.join("\n")),
        ("test records", records.join("\n")),
        ("records/ and routing/verdicts.jsonl", disk),
    ];
    // The test prompt is the request body's `"hi"` (research R2).
    let leaks = leaks_of(&places, &[TEST_KEY, key.as_str(), "OUTPUT-SENTINEL-T051", "\"content\":\"hi\""]);
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}
