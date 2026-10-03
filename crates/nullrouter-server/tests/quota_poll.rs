//! Quota polling (spec 005 T068; FR-018 – FR-022, SC-006, SC-007; research R11, R14). The
//! plugins are the bundled anthropic, grok-cli, opencode-go, opencode-zen and xai files with
//! every host pointed at one mock, which answers the quota routes with 9router-shaped samples,
//! the token endpoint with a fresh grant, and inference in every wire. Intervals are scaled
//! down (10 min → 0.9 s) and the retry is 0.6 s.
//!
//! Accounts: anthropic/max (sign-in), anthropic/api (key), grok-cli/work (sign-in, with a
//! refresh token), opencode-go/main and opencode-zen/main (keys), xai/main (sign-in).

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{bundled_at_mock, reply_by_wire};
use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::maintenance;
use nullrouter_engine::quota::poll::{PollErrorClass, PollTiming, QuotaPoll};
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::{MockUpstream, QuotaRoute, Received, Step};
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator;
use nullrouter_server::serve::{App, run};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "tok-SENTINEL";
const KEY: &str = "sk-quota-SENTINEL";
/// 10 min becomes 0.9 s.
const SCALE: f64 = 0.0015;
const RETRY: Duration = Duration::from_millis(600);

const CONFIG: &str = r#"allow_private_endpoints = true
[plugin_decisions]
anthropic = "replace"
xai = "replace"
"grok-cli" = "replace"
"opencode-go" = "replace"
"opencode-zen" = "replace"
"#;

type Answers = Arc<Mutex<HashMap<&'static str, Step>>>;

struct Quota {
    _dir: tempfile::TempDir,
    engine: Arc<Engine>,
    mock: MockUpstream,
    answers: Answers,
    polls: Arc<Mutex<Vec<(QuotaPoll, Instant)>>>,
    base: String,
    key: String,
    stop: CancellationToken,
}

impl Drop for Quota {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl Quota {
    /// Sets the answer of the route at `path` (no query) for every later call.
    fn set(&self, path: &'static str, step: Step) {
        self.answers.lock().unwrap().insert(path, step);
    }

    /// Runs the maintenance task until the test ends.
    fn upkeep(&self) {
        let stop = self.stop.clone();
        maintenance::spawn(self.engine.clone(), async move { stop.cancelled().await });
    }

    /// Polls of `provider/account` so far, with when they completed.
    fn polls_of(&self, provider: &str, account: &str) -> Vec<(QuotaPoll, Instant)> {
        self.polls
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.provider == provider && p.account == account)
            .cloned()
            .collect()
    }

    fn received(&self, path: &str) -> Vec<Received> {
        self.mock.received().into_iter().filter(|r| r.path_and_query.split('?').next() == Some(path)).collect()
    }

    async fn op(&self, req: Value) -> Value {
        operator::handle(&self.engine, &req).await
    }
}

fn quota_answer(path: &str) -> Option<Step> {
    QuotaRoute::ALL.into_iter().find(|r| r.path() == path).map(QuotaRoute::sample)
}

async fn setup() -> Quota {
    let mock = MockUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::write(home.join("config.toml"), CONFIG).unwrap();
    std::fs::create_dir(home.join("plugins")).unwrap();
    for id in ["anthropic", "grok-cli", "opencode-go", "opencode-zen", "xai"] {
        std::fs::write(home.join(format!("plugins/{id}.toml")), bundled_at_mock(&mock, id)).unwrap();
    }
    let signin = |p: &str, n: &str, o: u32| {
        format!("[[account]]\nprovider = \"{p}\"\nname = \"{n}\"\nkind = \"signin\"\norder = {o}\n")
    };
    let key = |p: &str, n: &str, o: u32| {
        format!("[[account]]\nprovider = \"{p}\"\nname = \"{n}\"\nsecret = \"{KEY}-{p}\"\norder = {o}\n")
    };
    let file = format!(
        "schema = 2\n{}{}{}{}{}{}",
        signin("anthropic", "max", 0),
        key("anthropic", "api", 1),
        signin("grok-cli", "work", 0),
        key("opencode-go", "main", 0),
        key("opencode-zen", "main", 0),
        signin("xai", "main", 0),
    );
    nullrouter_engine::files::write_private(&home.join(accounts::FILE), &file).unwrap();
    let mut tokens = String::from("schema = 1\n");
    for (p, n) in [("anthropic", "max"), ("grok-cli", "work"), ("xai", "main")] {
        tokens += &format!(
            "[[token]]\nprovider = \"{p}\"\nname = \"{n}\"\naccess_token = \"{TOKEN}-{p}\"\nrefresh_token = \"refresh-{p}\"\nexpires_at = \"2099-01-01T00:00:00Z\"\nsigned_in_at = \"2026-10-01T00:00:00Z\"\nhosts = [\"127.0.0.1\"]\nclaims = {{ email = \"{n}@example.com\", user_id = \"user-{n}\" }}\n"
        );
    }
    nullrouter_engine::files::write_private(&nullrouter_engine::tokens::path(home), &tokens).unwrap();
    let mut keys = Keys::default();
    let (agent_key, _) = keys.issue("laptop", None).unwrap();
    nullrouter_engine::files::write_private(&home.join(keys::FILE), &keys.to_toml()).unwrap();

    let answers: Answers = Arc::default();
    let a = answers.clone();
    mock.respond(move |r: &Received| {
        let path = r.path_and_query.split('?').next().unwrap_or_default().to_owned();
        if let Some(step) = a.lock().unwrap().get(path.as_str()) {
            return step.clone();
        }
        if path.ends_with("/token") {
            return Step::json(
                200,
                json!({"access_token": format!("{TOKEN}-fresh"), "refresh_token": "refresh-2", "expires_in": 3600}),
            );
        }
        quota_answer(&path).unwrap_or_else(|| reply_by_wire(r))
    });

    let (engine, report) = Engine::open_parity(OperatorHome::new(home)).unwrap();
    let r = &report.registry;
    assert!(r.unsupported.is_empty() && r.skipped.is_empty() && r.pending_conflicts.is_empty(), "{report:#?}");
    let engine = Arc::new(engine);
    engine.quota.set_timing(PollTiming {
        scale: SCALE,
        retry_after: RETRY,
        jitter: 0.1,
        timeout: Duration::from_secs(5),
    });
    let polls: Arc<Mutex<Vec<(QuotaPoll, Instant)>>> = Arc::default();
    let p = polls.clone();
    engine.quota.on_poll(Arc::new(move |poll: &QuotaPoll| p.lock().unwrap().push((poll.clone(), Instant::now()))));

    let app = App::new(engine.clone()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let s = stop.clone();
    tokio::spawn(run(app, listener, async move { s.cancelled().await }));
    Quota { _dir: dir, engine, mock, answers, polls, base, key: agent_key, stop }
}

fn bearer(r: &Received) -> String {
    r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned()
}

fn header(r: &Received, name: &str) -> String {
    r.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned()
}

/// Gaps between consecutive polls.
fn gaps(polls: &[(QuotaPoll, Instant)]) -> Vec<Duration> {
    polls.windows(2).map(|w| w[1].1 - w[0].1).collect()
}

#[tokio::test]
async fn polls_run_with_no_traffic_at_the_interval_with_jitter() {
    let q = setup().await;
    q.upkeep();
    tokio::time::sleep(Duration::from_millis(4200)).await;
    let interval = Duration::from_secs(600).mul_f64(SCALE);
    for (p, n) in [("anthropic", "max"), ("grok-cli", "work"), ("opencode-go", "main"), ("opencode-zen", "main")] {
        let polls = q.polls_of(p, n);
        assert!(polls.len() >= 4, "{p}/{n}: {} polls in 4.2 s", polls.len());
        assert!(polls.iter().all(|(p, _)| p.ok()), "{p}/{n}: {polls:?}");
        // ±10% jitter, plus scheduling slack.
        for g in gaps(&polls) {
            assert!(
                g >= interval.mul_f64(0.9) - Duration::from_millis(30)
                    && g <= interval.mul_f64(1.1) + Duration::from_millis(250),
                "{p}/{n}: gap {g:?} vs interval {interval:?}"
            );
        }
    }
    // Quota not reported: anthropic's key account ([quota] is for sign-in accounts) and xai.
    assert!(q.polls_of("anthropic", "api").is_empty() && q.polls_of("xai", "main").is_empty());
    let list = q.op(json!({"op": "quota.list"})).await;
    let reported: Vec<(String, bool)> = list["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (format!("{}/{}", a["provider"].as_str().unwrap(), a["name"].as_str().unwrap()), a["reported"] == true)
        })
        .collect();
    assert_eq!(
        reported,
        [
            ("anthropic/max".to_owned(), true),
            ("anthropic/api".to_owned(), false),
            ("grok-cli/work".to_owned(), true),
            ("opencode-go/main".to_owned(), true),
            ("opencode-zen/main".to_owned(), true),
            ("xai/main".to_owned(), false),
        ]
    );

    // Each poll carries the account's credential (key accounts are polled too) and the
    // declared and identity headers.
    let usage = q.received(QuotaRoute::AnthropicUsage.path());
    assert!(usage.iter().all(|r| bearer(r) == format!("Bearer {TOKEN}-anthropic")), "the sign-in token, never the key");
    assert_eq!(header(&usage[0], "anthropic-beta"), "oauth-2025-04-20");
    assert_eq!(header(&usage[0], "user-agent"), "claude-cli/2.1.280 (external, sdk-cli)");
    let billing = &q.received(QuotaRoute::GrokBilling.path())[0];
    assert_eq!(billing.path_and_query, "/v1/billing?format=credits");
    assert_eq!(bearer(billing), format!("Bearer {TOKEN}-grok-cli"));
    assert_eq!(header(billing, "x-xai-token-auth"), "xai-grok-cli");
    assert_eq!(header(billing, "user-agent"), "grok-shell/0.2.99 (linux; x86_64)");
    assert_eq!(header(billing, "x-email"), "work@example.com");
    let go = &q.received(QuotaRoute::OpencodeGo.path())[0];
    assert_eq!(bearer(go), format!("Bearer {KEY}-opencode-go"));
    assert!(go.headers.get("x-grok-agent-id").is_none());
    assert_eq!(bearer(&q.received(QuotaRoute::OpencodeZen.path())[0]), format!("Bearer {KEY}-opencode-zen"));
    // The live model list was read at start with the sign-in account.
    let models = q.received(QuotaRoute::Models.path());
    assert!(!models.is_empty() && bearer(&models[0]) == format!("Bearer {TOKEN}-grok-cli"));
    assert_eq!(header(&models[0], "x-grok-client-mode"), "headless");
}

#[tokio::test]
async fn shown_values_equal_the_reported_ones() {
    let q = setup().await;
    let a = q.op(json!({"op": "quota.poll", "provider": "anthropic", "name": "max"})).await;
    assert_eq!(a["ok"], true, "{a}");
    let windows = a["poll"]["windows"].as_array().unwrap();
    let by_name =
        |n: &str| windows.iter().find(|w| w["name"] == n).unwrap_or_else(|| panic!("{n}: {windows:?}")).clone();
    assert_eq!(
        by_name("5-hour"),
        json!({"name": "5-hour", "unit": "percent", "used": 87.0, "limit": 100.0, "remaining": 13.0, "resets_at": "2026-10-02T18:00:00.000Z"})
    );
    assert_eq!(by_name("weekly")["used"], 40.0);
    assert_eq!(by_name("weekly opus")["used"], 12.5);
    assert_eq!(by_name("weekly fable")["used"], 30.0);

    let a = q.op(json!({"op": "quota.poll", "provider": "grok-cli", "name": "work"})).await;
    let w = &a["poll"]["windows"][0];
    assert_eq!(
        (w["name"].as_str(), w["used"].as_f64(), w["limit"].as_f64()),
        (Some("monthly included"), Some(1200.0), Some(2500.0))
    );

    // Changed values show after the next poll; over-limit usage is shown uncapped.
    q.set(
        QuotaRoute::OpencodeGo.path(),
        Step::json(200, json!({"usage": {"rolling": {"percent": 130, "resetsAt": "2026-09-04T14:28:02.617Z"}}})),
    );
    let a = q.op(json!({"op": "quota.poll", "provider": "opencode-go", "name": "main"})).await;
    assert_eq!(
        a["poll"]["windows"],
        json!([{"name": "rolling", "unit": "percent", "used": 130.0, "limit": 100.0, "remaining": 0.0, "resets_at": "2026-09-04T14:28:02.617Z"}])
    );

    // The gRPC-web fallback when the billing read has no window.
    q.set(QuotaRoute::GrokBilling.path(), Step::json(200, json!({"config": {}})));
    let a = q.op(json!({"op": "quota.poll", "provider": "grok-cli", "name": "work"})).await;
    let w = &a["poll"]["windows"][0];
    assert_eq!((w["name"].as_str(), w["unit"].as_str()), (Some("weekly SuperGrok"), Some("percent")));
    assert!((w["used"].as_f64().unwrap() - 35.0).abs() < 1e-3, "{w}");
    let credits = q.received(QuotaRoute::GrokCredits.path());
    assert_eq!(credits.len(), 1);
    assert_eq!(&credits[0].body[..], &[0u8; 5]);
    assert_eq!(header(&credits[0], "content-type"), "application/grpc-web+proto");

    for (p, n) in [("anthropic", "api"), ("xai", "main")] {
        let a = q.op(json!({"op": "quota.poll", "provider": p, "name": n})).await;
        assert_eq!(a, json!({"ok": false, "error": format!("{p}/{n}: quota not reported")}));
    }
    // No token crosses the socket.
    let list = q.op(json!({"op": "quota.list"})).await.to_string();
    assert!(!list.contains("SENTINEL"), "{list}");
}

#[tokio::test]
async fn a_failed_poll_retries_once_then_waits_and_keeps_the_last_good_values() {
    let q = setup().await;
    let good = q.engine.poll_quota("opencode-zen", "main").await.unwrap();
    assert!(good.ok());
    q.set(QuotaRoute::OpencodeZen.path(), Step::json(500, json!({"error": "down"})));
    q.upkeep();
    tokio::time::sleep(Duration::from_millis(2750)).await;
    let polls = q.polls_of("opencode-zen", "main");
    // good (by hand) → failed at ~0.9 s → its retry ~0.6 s later → the next interval.
    assert!(polls.len() >= 4, "{polls:?}");
    assert!(!polls[1].0.ok() && !polls[1].0.retry);
    assert!(!polls[2].0.ok() && polls[2].0.retry, "one retry");
    assert!(!polls[3].0.retry, "then the interval: {polls:?}");
    let g = gaps(&polls);
    assert!(
        g[1] >= RETRY - Duration::from_millis(30) && g[1] < RETRY + Duration::from_millis(250),
        "retry after {:?}",
        g[1]
    );
    assert!(g[2] >= Duration::from_millis(780), "after the retry: the interval, not another retry ({:?})", g[2]);

    // FR-021: the last good values stay, with their own time, and the failure with its own.
    let a = q.op(json!({"op": "quota.list", "provider": "opencode-zen"})).await;
    let acc = &a["accounts"][0];
    assert_eq!(acc["latest"]["windows"][0]["name"], "rolling");
    assert_eq!(acc["latest"]["at"], serde_json::to_value(&good).unwrap()["at"]);
    assert_eq!(acc["last_failure"]["error"]["class"], "status");
    assert_eq!(acc["last_failure"]["error"]["summary"], "HTTP 500");
    assert_eq!(acc["last_failure"]["windows"], json!([]));
}

#[tokio::test]
async fn a_429_skips_the_retry() {
    let q = setup().await;
    q.engine.poll_quota("opencode-go", "main").await.unwrap();
    q.set(QuotaRoute::OpencodeGo.path(), Step::rate_limited(1, json!({"error": "slow down"})));
    q.upkeep();
    tokio::time::sleep(Duration::from_millis(2300)).await;
    let polls = q.polls_of("opencode-go", "main");
    assert!(polls.len() >= 3, "{polls:?}");
    assert_eq!(polls[1].0.error.as_ref().unwrap().class, PollErrorClass::RateLimited);
    assert!(polls.iter().all(|(p, _)| !p.retry), "no retry after a 429");
    assert!(gaps(&polls)[1..].iter().all(|g| *g >= Duration::from_millis(780)), "{:?}", gaps(&polls));
}

#[tokio::test]
async fn a_401_on_a_sign_in_account_refreshes_and_retries() {
    let q = setup().await;
    q.mock.on(QuotaRoute::GrokBilling.path(), [Step::json(401, json!({"error": "expired"}))]);
    let poll = q.engine.poll_quota("grok-cli", "work").await.unwrap();
    assert!(poll.ok(), "{poll:?}");
    let billing = q.received(QuotaRoute::GrokBilling.path());
    assert_eq!(
        billing.iter().map(bearer).collect::<Vec<_>>(),
        [format!("Bearer {TOKEN}-grok-cli"), format!("Bearer {TOKEN}-fresh")]
    );
    let token = q.received("/oauth2/token");
    assert_eq!(token.len(), 1);
    assert!(String::from_utf8_lossy(&token[0].body).contains("refresh_token=refresh-grok-cli"));

    // The live model list does the same (9router's refresh-and-retry flow).
    q.mock.on(QuotaRoute::Models.path(), [Step::json(401, json!({"error": "expired"}))]);
    let n = q.engine.fetch_live_models("grok-cli").await;
    assert!(n.is_ok(), "{n:?}");
    let models = q.received(QuotaRoute::Models.path());
    assert_eq!(models.len(), 2);
    // A key account's 401 is a failure, not a refresh.
    q.set(QuotaRoute::OpencodeGo.path(), Step::json(401, json!({"error": "bad key"})));
    let poll = q.engine.poll_quota("opencode-go", "main").await.unwrap();
    assert_eq!(poll.error.unwrap().class, PollErrorClass::Rejected);
    assert_eq!(q.received("/oauth2/token").len(), 2, "one more refresh: the model list's");
}

#[tokio::test]
async fn quota_interval_changes_only_that_account() {
    let q = setup().await;
    let path = q._dir.path().join(accounts::FILE);
    let mut list = Accounts::load(&path).unwrap();
    let every = list.set_poll_interval("opencode-go", "main", Some(Duration::from_secs(1800))).unwrap();
    assert_eq!(every, Duration::from_secs(1800));
    list.save().unwrap();
    assert_eq!(q.op(json!({"op": "reload"})).await["ok"], true);
    q.upkeep();
    tokio::time::sleep(Duration::from_millis(3300)).await;
    let (go, zen) = (q.polls_of("opencode-go", "main"), q.polls_of("opencode-zen", "main"));
    assert_eq!(go.len(), 2, "polled at start and once after 2.7 s: {go:?}");
    assert!(gaps(&go)[0] >= Duration::from_millis(2400), "{:?}", gaps(&go));
    assert!(zen.len() >= 3, "the others keep the default: {zen:?}");
    let a = q.op(json!({"op": "quota.list", "provider": "opencode-go", "name": "main"})).await;
    assert_eq!(a["accounts"][0]["interval_s"], 1800);
}

#[tokio::test]
async fn an_account_at_zero_percent_is_still_tried_first() {
    let q = setup().await;
    q.set(
        QuotaRoute::AnthropicUsage.path(),
        Step::json(200, json!({"five_hour": {"utilization": 100, "resets_at": "2026-10-02T18:00:00Z"}})),
    );
    let poll = q.engine.poll_quota("anthropic", "max").await.unwrap();
    assert_eq!(poll.windows[0].remaining, Some(0.0));
    let body = json!({"model": "anthropic/claude-sonnet-4-20250514", "max_tokens": 16, "messages": [{"role": "user", "content": "hi"}]});
    let r = reqwest::Client::new()
        .post(format!("{}/v1/messages", q.base))
        .header("x-api-key", &q.key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let sent = q.received("/v1/messages");
    assert_eq!(sent.len(), 1);
    assert_eq!(bearer(&sent[0]), format!("Bearer {TOKEN}-anthropic"), "the 0% account served, first in order");
}

#[tokio::test]
async fn the_live_model_list_joins_the_static_one_in_every_style() {
    let q = setup().await;
    let listed = |shape: &'static str| {
        let (base, key) = (q.base.clone(), q.key.clone());
        async move {
            let path = if shape == "gemini" { "/v1beta/models" } else { "/v1/models" };
            let mut req =
                reqwest::Client::new().get(format!("{base}{path}")).bearer_auth(&key).header("x-goog-api-key", &key);
            if shape == "anthropic" {
                req = req.header("x-api-key", &key).header("anthropic-version", "2023-06-01");
            }
            let v: Value = serde_json::from_slice(&req.send().await.unwrap().bytes().await.unwrap()).unwrap();
            match shape {
                "gemini" => pairs(&v["models"], "name")
                    .into_iter()
                    .map(|(i, t)| (i.trim_start_matches("models/").to_owned(), t))
                    .collect::<Vec<_>>(),
                _ => pairs(&v["data"], "id"),
            }
        }
    };
    fn pairs(list: &Value, key: &str) -> Vec<(String, String)> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|e| (e[key].as_str().unwrap().to_owned(), e["nullrouter"]["type"].as_str().unwrap().to_owned()))
            .collect()
    }
    let has = |l: &[(String, String)], id: &str| l.iter().any(|(i, t)| i == id && t == "text");

    // A failed read: the static list alone.
    q.set(QuotaRoute::Models.path(), Step::json(500, json!({"error": "down"})));
    assert!(q.engine.fetch_live_models("grok-cli").await.is_err());
    let l = listed("openai").await;
    assert!(has(&l, "grok-cli/grok-4.5") && !has(&l, "grok-cli/grok-live"), "{l:?}");

    q.set(
        QuotaRoute::Models.path(),
        Step::json(200, json!({"data": [{"id": "grok-build"}, {"id": "grok-live", "display_name": "Grok Live"}]})),
    );
    assert_eq!(q.engine.fetch_live_models("grok-cli").await.unwrap(), 2);
    for shape in ["openai", "anthropic", "gemini"] {
        let l = listed(shape).await;
        assert!(has(&l, "grok-cli/grok-live"), "{shape}: {l:?}");
        assert!(has(&l, "grok-cli/grok-4.5") && has(&l, "grok-cli/grok-build"), "{shape}: static kept");
        assert_eq!(l.iter().filter(|(i, _)| i == "grok-cli/grok-build").count(), 1, "{shape}: no duplicate");
    }
    // A later failure keeps the last good list.
    q.set(QuotaRoute::Models.path(), Step::json(500, json!({"error": "down"})));
    assert!(q.engine.fetch_live_models("grok-cli").await.is_err());
    assert!(has(&listed("openai").await, "grok-cli/grok-live"));
}
