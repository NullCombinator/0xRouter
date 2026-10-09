//! Video jobs with a declared poll URL and job mapping (slice 005 T099, FR-008): xAI's
//! shape. Submit answers `{request_id}`, polls go to `poll_url`, the status is read through
//! `job`, and the content is a download from the declared `video.url`, with the account's
//! secret sent only to a host the account is bound to.

mod common;

use std::time::Duration;

use common::{SECRET, Server, bundled_at_mock, server_with, signin_server};
use nullrouter_engine::records::Outcome;
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

/// `vidco`: a key-account provider shaped like xAI's video API.
fn vidco(mock: &MockUpstream) -> (&'static str, String) {
    let toml = format!(
        r#"schema = 2
id = "vidco"
category = "apikey"
[auth]
kind = "apikey"
[endpoints.video]
url = "{submit}"
wire = "openai-chat"
poll_url = "{poll}"
job = {{ id = "request_id", status = "status", status_map = {{ pending = "queued", processing = "in_progress", done = "completed" }}, content_url = "video.url", error = "error.message" }}
[[models]]
id = "vid"
kind = "video"
"#,
        submit = mock.url("/v1/videos/generations"),
        poll = mock.url("/v1/videos/{id}"),
    );
    ("vidco", toml)
}

fn post(s: &Server, body: &Value) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(format!("{}/v1/videos", s.base))
        .bearer_auth(&s.key)
        .header("content-type", "application/json")
        .body(body.to_string())
}

fn get(s: &Server, path: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new().get(format!("{}{path}", s.base)).bearer_auth(&s.key)
}

async fn json(r: reqwest::Response) -> Value {
    serde_json::from_slice(&r.bytes().await.unwrap()).unwrap()
}

/// Submits `model`; returns the record id and the `vj_` id.
async fn submit(s: &Server, model: &str) -> (String, String) {
    let r = post(s, &json!({"model": model, "prompt": "neon city", "duration": 8})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let rec = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let job = json(r).await;
    assert_eq!(job["status"], "queued", "{job}");
    let vj = job["id"].as_str().unwrap().to_owned();
    assert!(vj.starts_with("vj_"), "{job}");
    (rec, vj)
}

async fn outcome(s: &Server, rec: &str, want: Outcome) {
    for _ in 0..100 {
        if s.engine.records.get(rec).unwrap().outcome == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(s.engine.records.get(rec).unwrap().outcome, want);
}

#[tokio::test]
async fn submit_poll_and_download_from_the_declared_url() {
    let s = server_with(|m| vec![vidco(m)]).await;
    let file = s.mock.url("/files/out.mp4");
    s.mock.on("/files/out.mp4", [Step::binary("video/mp4", &b"MP4-bytes"[..])]);
    s.mock.on(
        "/v1/videos/req-1",
        [
            Step::json(200, json!({"status": "processing", "progress": 55})),
            Step::json(200, json!({"status": "done", "video": {"url": file, "duration": 8}})),
        ],
    );
    s.mock.on("/v1/videos/generations", [Step::json(200, json!({"request_id": "req-1"}))]);

    let (rec, vj) = submit(&s, "vidco/vid").await;
    let sent = s.mock.received()[0].json();
    assert_eq!((sent["model"].as_str(), sent["duration"].as_u64()), (Some("vid"), Some(8)), "{sent}");
    assert_eq!(s.engine.records.get(&rec).unwrap().job.unwrap().upstream_id, "req-1");

    let poll = json(get(&s, &format!("/v1/videos/{vj}")).send().await.unwrap()).await;
    assert_eq!(poll["status"], "in_progress", "{poll}");
    let poll = json(get(&s, &format!("/v1/videos/{vj}")).send().await.unwrap()).await;
    assert_eq!(poll["status"], "completed", "{poll}");
    assert_eq!(poll["id"], vj.as_str());
    assert!(!poll.to_string().contains("/files/out.mp4"), "the download URL stays hidden: {poll}");

    let content = get(&s, &format!("/v1/videos/{vj}/content")).send().await.unwrap();
    assert_eq!(content.status(), 200);
    assert_eq!(content.headers()["content-type"], "video/mp4");
    assert_eq!(&content.bytes().await.unwrap()[..], b"MP4-bytes");
    let got = s.mock.received();
    let paths: Vec<&str> = got.iter().map(|r| r.path_and_query.as_str()).collect();
    assert_eq!(paths, ["/v1/videos/generations", "/v1/videos/req-1", "/v1/videos/req-1", "/files/out.mp4"]);
    assert!(
        got.iter().all(|r| r.headers["authorization"] == format!("Bearer {SECRET}")),
        "the mock's host is the account's own"
    );
    outcome(&s, &rec, Outcome::Succeeded).await;
}

#[tokio::test]
async fn a_download_on_a_foreign_host_gets_no_auth_header() {
    let s = server_with(|m| vec![vidco(m)]).await;
    // Same listener, other host name: `localhost` is not a host the account was added for.
    let foreign = format!("http://localhost:{}/files/out.mp4", s.mock.addr().port());
    s.mock.on("/files/out.mp4", [Step::binary("video/mp4", &b"MP4"[..])]);
    s.mock.on("/v1/videos/req-2", [Step::json(200, json!({"status": "done", "video": {"url": foreign}}))]);
    s.mock.on("/v1/videos/generations", [Step::json(200, json!({"request_id": "req-2"}))]);

    let (_, vj) = submit(&s, "vidco/vid").await;
    // No poll first: the content fetch polls for the URL itself.
    let content = get(&s, &format!("/v1/videos/{vj}/content")).send().await.unwrap();
    assert_eq!(content.status(), 200);
    assert_eq!(&content.bytes().await.unwrap()[..], b"MP4");
    let download = s.mock.received().pop().unwrap();
    assert_eq!(download.path_and_query, "/files/out.mp4");
    assert!(download.headers.get("authorization").is_none(), "{:?}", download.headers);
    assert!(!format!("{:?}", download.headers).contains(SECRET));
}

#[tokio::test]
async fn a_redirecting_download_is_not_followed() {
    let s = server_with(|m| vec![vidco(m)]).await;
    let file = s.mock.url("/files/moved.mp4");
    s.mock.on("/files/moved.mp4", [Step::json(302, json!({})).with_header("location", "http://169.254.169.254/")]);
    s.mock.on("/v1/videos/req-4", [Step::json(200, json!({"status": "done", "video": {"url": file}}))]);
    s.mock.on("/v1/videos/generations", [Step::json(200, json!({"request_id": "req-4"}))]);
    let (_, vj) = submit(&s, "vidco/vid").await;
    let content = get(&s, &format!("/v1/videos/{vj}/content")).send().await.unwrap();
    assert_eq!(content.status(), 502);
    assert_eq!(s.mock.received().len(), 3, "nothing past the redirect");
}

#[tokio::test]
async fn a_failed_job_fails_its_record() {
    let s = server_with(|m| vec![vidco(m)]).await;
    let failed = json!({"status": "failed", "error": {"code": "internal_error", "message": "render crashed"}});
    // The content fetch polls again for a URL the failed job never got.
    s.mock.on("/v1/videos/req-3", [Step::json(200, failed.clone()), Step::json(200, failed)]);
    s.mock.on("/v1/videos/generations", [Step::json(200, json!({"request_id": "req-3"}))]);
    let (rec, vj) = submit(&s, "vidco/vid").await;
    let poll = json(get(&s, &format!("/v1/videos/{vj}")).send().await.unwrap()).await;
    assert_eq!(poll["status"], "failed", "{poll}");
    assert_eq!(poll["error"], "render crashed", "{poll}");
    outcome(&s, &rec, Outcome::Failed).await;
    let content = get(&s, &format!("/v1/videos/{vj}/content")).send().await.unwrap();
    assert_eq!(content.status(), 409, "a failed job has no content");
}

/// The bundled xai plugin, on a sign-in account: the token goes to submit, polls and the
/// download on the account's bound host.
#[tokio::test]
async fn a_signed_in_xai_account_runs_a_video_job() {
    let s = signin_server(
        |m| vec![("xai", bundled_at_mock(m, "xai"))],
        &[("xai", "main")],
        "[plugin_decisions]\nxai = \"replace\"\n",
    )
    .await;
    let file = s.mock.url("/files/v.mp4");
    s.mock.on("/files/v.mp4", [Step::binary("video/mp4", &b"MP4-xai"[..])]);
    s.mock.on(
        "/v1/videos/req-x",
        [
            Step::json(200, json!({"status": "pending", "progress": 10})),
            Step::json(200, json!({"status": "done", "video": {"url": file, "duration": 8}})),
        ],
    );
    s.mock.on("/v1/videos/generations", [Step::json(200, json!({"request_id": "req-x"}))]);

    let (rec, vj) = submit(&s, "xai/grok-imagine-video").await;
    assert_eq!(s.mock.received()[0].json()["model"], "grok-imagine-video");
    let poll = json(get(&s, &format!("/v1/videos/{vj}")).send().await.unwrap()).await;
    assert_eq!(poll["status"], "queued", "{poll}");
    let poll = json(get(&s, &format!("/v1/videos/{vj}")).send().await.unwrap()).await;
    assert_eq!(poll["status"], "completed", "{poll}");
    let content = get(&s, &format!("/v1/videos/{vj}/content")).send().await.unwrap();
    assert_eq!(&content.bytes().await.unwrap()[..], b"MP4-xai");
    let got = s.mock.received();
    let paths: Vec<&str> = got.iter().map(|r| r.path_and_query.as_str()).collect();
    assert_eq!(paths, ["/v1/videos/generations", "/v1/videos/req-x", "/v1/videos/req-x", "/files/v.mp4"]);
    assert!(got.iter().all(|r| r.headers["authorization"] == format!("Bearer {SECRET}-xai-main")));
    outcome(&s, &rec, Outcome::Succeeded).await;
}

/// Spec 013: the submit's phases end when 0router answers with the job id; polls and the
/// download add no attempt and change nothing in the submit's timing.
#[tokio::test]
async fn polling_a_job_adds_nothing_to_the_submits_phases() {
    use nullrouter_engine::phases::{self, Phase, PhaseValue};

    let s = server_with(|m| vec![vidco(m)]).await;
    let file = s.mock.url("/files/out.mp4");
    s.mock.on("/files/out.mp4", [Step::binary("video/mp4", &b"MP4-bytes"[..])]);
    s.mock.on(
        "/v1/videos/req-1",
        [
            Step::json(200, json!({"status": "processing"})),
            Step::json(200, json!({"status": "done", "video": {"url": file, "duration": 8}})),
        ],
    );
    s.mock.on("/v1/videos/generations", [Step::json(200, json!({"request_id": "req-1"}))]);

    let (rec, vj) = submit(&s, "vidco/vid").await;
    for _ in 0..100 {
        if s.engine.records.get(&rec).unwrap().attempts.first().is_some_and(|a| a.ended.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let before = s.engine.records.get(&rec).unwrap();
    let timing = before.attempts[0].timing.clone().expect("the submit's attempt is timed");
    assert!(timing.first_output.is_some(), "the job id is the answer");
    assert_eq!(timing.first_output, timing.upstream_done, "a whole answer");
    assert_eq!(phases::of(&before)[0].value(Phase::Generation), PhaseValue::NotApplicable);

    for _ in 0..2 {
        json(get(&s, &format!("/v1/videos/{vj}")).send().await.unwrap()).await;
    }
    get(&s, &format!("/v1/videos/{vj}/content")).send().await.unwrap().bytes().await.unwrap();
    outcome(&s, &rec, Outcome::Succeeded).await;

    let after = s.engine.records.get(&rec).unwrap();
    assert_eq!(after.attempts.len(), 1, "polls are not attempts");
    assert_eq!(serde_json::to_value(&after.attempts[0].timing).unwrap(), serde_json::to_value(Some(timing)).unwrap());
}

#[tokio::test]
async fn a_paused_proxy_stops_a_job_poll_before_anything_is_sent() {
    use nullrouter_engine::accounts::{self, Accounts};
    use nullrouter_engine::connection::fingerprints;
    use nullrouter_engine::testkit::MockProxy;

    let s = server_with(|m| vec![vidco(m)]).await;
    let proxy = MockProxy::start().await;
    nullrouter_engine::files::write_private(
        &s.home().join("proxies.toml"),
        &format!("schema = 1\n\n[[proxy]]\nname = \"eu\"\nurl = \"http://{}\"\n", proxy.addr()),
    )
    .unwrap();
    let mut list = Accounts::load(&s.home().join(accounts::FILE)).unwrap();
    list.set_proxy("vidco", "main", Some("eu".into())).unwrap();
    list.save().unwrap();
    s.engine.reload().await.unwrap();

    s.mock.on("/v1/videos/generations", [Step::json(200, json!({"request_id": "req-1"}))]);
    s.mock.on("/v1/videos/req-1", [Step::json(200, json!({"status": "processing"}))]);
    let (_, vj) = submit(&s, "vidco/vid").await;
    let first = json(get(&s, &format!("/v1/videos/{vj}")).send().await.unwrap()).await;
    assert_eq!(first["status"], "in_progress", "{first}");
    assert!(proxy.carried() >= 2, "the submit and the poll used the account's proxy");

    let print = fingerprints(&s.engine.snapshot()).remove("eu").unwrap();
    s.engine.proxy_board.pause("eu", "connect to proxy failed", &print);
    let (carried, seen) = (proxy.carried(), s.mock.received().len());
    let r = get(&s, &format!("/v1/videos/{vj}")).send().await.unwrap();
    assert_eq!(r.status(), 502);
    let body = r.text().await.unwrap();
    assert!(body.contains("proxy eu paused"), "{body}");
    assert_eq!((proxy.carried(), s.mock.received().len()), (carried, seen), "nothing was sent");
}
