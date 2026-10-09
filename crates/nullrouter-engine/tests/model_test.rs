//! Spec 011 T014, US1 scenarios 1–8, SC-008: one pair's test end to end against a scripted
//! upstream. Each test calls exactly one account, its record is marked as a test, and its
//! verdict lands on the board.

mod common;

use std::time::{Duration, Instant};

use common::*;
use nullrouter_engine::records::Outcome;
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_engine::tests::{self, TestResult};
use nullrouter_engine::verdict::{Pair, Rejection, Source, State};
use serde_json::json;
use tokio_util::sync::CancellationToken;

const NO_RETRY: &str = "retry = { 429 = { retries = 0 }, 503 = { retries = 0 } }";

/// `mediaco`: every type on the openai-chat wire, one model each, and one `[[rejections]]` rule.
fn mediaco(mock: &MockUpstream) -> String {
    let ep = |ty: &str, path: &str, extra: &str| {
        format!("[endpoints.{ty}]\nurl = \"{}\"\nwire = \"openai-chat\"\n{NO_RETRY}\n{extra}\n", mock.url(path))
    };
    [
        "schema = 2\nid = \"mediaco\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n".to_owned(),
        ep("text", "/media/chat/completions", ""),
        ep("embeddings", "/media/embeddings", ""),
        ep("tts", "/media/audio/speech", "voices = [\"alloy\"]"),
        ep("stt", "/media/audio/transcriptions", ""),
        ep("image", "/media/images", ""),
        ep("video", "/media/videos", ""),
        "[[rejections]]\nstatus = 410\nbody_contains = \"model_retired\"\nreason = \"model_not_found\"\n".to_owned(),
        "[[models]]\nid = \"m1\"\n".to_owned(),
        "[[models]]\nid = \"emb\"\nkind = \"embedding\"\n[[models]]\nid = \"say\"\nkind = \"tts\"\n".to_owned(),
        "[[models]]\nid = \"hear\"\nkind = \"stt\"\n[[models]]\nid = \"img\"\nkind = \"image\"\n".to_owned(),
        "[[models]]\nid = \"vid\"\nkind = \"video\"\n".to_owned(),
    ]
    .concat()
}

async fn media(accounts: &[(&str, &str)]) -> Setup {
    setup(|m| vec![("mediaco", mediaco(m))], accounts, "[tests.timeout]\ntext = \"5s\"\n").await
}

async fn tested(s: &Setup, target: &str, account: Option<&str>) -> TestResult {
    let st = s.engine.snapshot();
    let planned = tests::expand(&s.engine, &st, Some(target), account).unwrap();
    assert_eq!(planned.len(), 1, "{planned:?}");
    tests::run_pair(&s.engine, &planned[0], Source::Test, "tr_t", &CancellationToken::new()).await.unwrap()
}

fn rejected(status: u16, message: &str) -> Step {
    Step::json(status, json!({"error": {"message": message, "type": "invalid_request_error"}}))
}

#[tokio::test]
async fn every_type_passes_on_a_good_answer() {
    let s = media(&[("mediaco", "main")]).await;
    s.mock.on("/media/chat", [ok()]);
    s.mock.on(
        "/media/embeddings",
        [Step::json(200, json!({"object": "list", "data": [{"object": "embedding", "index": 0, "embedding": [0.5]}], "model": "emb"}))],
    );
    s.mock.on("/media/audio/speech", [Step::binary("audio/mpeg", &b"ID3"[..])]);
    // Silence transcribes to nothing: still a PASS.
    s.mock.on("/media/audio/transcriptions", [Step::json(200, json!({"text": ""}))]);
    s.mock.on("/media/images", [Step::json(200, json!({"created": 1, "data": [{"url": "https://cdn.example.com/i.png"}]}))]);
    s.mock.on("/media/videos/up_v1/content", [Step::binary("video/mp4", &b"MP4"[..])]);
    s.mock.on("/media/videos", [Step::accepted(json!({"id": "up_v1", "object": "video", "status": "completed"}))]);

    for model in ["m1", "emb", "say", "hear", "img", "vid"] {
        let r = tested(&s, &format!("mediaco/{model}"), None).await;
        assert_eq!(r.state, Some(State::Pass), "{model}: {r:?}");
        let v = s.engine.verdicts.get(&Pair::new("mediaco", "main", model)).unwrap();
        assert_eq!((v.state, v.source), (State::Pass, Source::Test), "{model}");
        assert!(v.basis.secret.as_deref().is_some_and(|d| d.starts_with("sha256:")), "{model}: {:?}", v.basis);
        assert!(v.basis.plugin.starts_with("sha256:"));
        let rec = settled(&s, r.record.as_deref().unwrap()).await;
        assert_eq!(rec.test.as_ref().map(|t| (t.run.as_str(), t.source)), Some(("tr_t", Source::Test)), "{model}");
        assert!(rec.agent.is_none(), "a test record carries no agent");
    }
    let sent: Vec<String> = paths(&s);
    assert_eq!(sent.iter().filter(|p| p.starts_with("/media/chat")).count(), 1);
    assert_eq!(s.engine.history.tally.get("mediaco", "main")["m1"].requests, 1, "SC-008: tests are tallied");
}

#[tokio::test]
async fn definitive_rejections_are_broken() {
    let s = media(&[("mediaco", "main")]).await;
    let cases = [
        (rejected(404, "The model 'm1' does not exist"), Rejection::ModelNotFound),
        (rejected(403, "Your plan does not include access to model m1"), Rejection::ModelNotAvailable),
        (rejected(400, "This model does not support chat completions"), Rejection::TypeNotSupported),
        (rejected(410, "{\"code\":\"model_retired\"}"), Rejection::Plugin("model_not_found".into())),
    ];
    for (step, want) in cases {
        s.mock.on("/media/chat", [step]);
        let r = tested(&s, "mediaco/m1", None).await;
        assert_eq!((r.state, r.rejection.clone()), (Some(State::Broken), Some(want.clone())), "{r:?}");
        let v = s.engine.verdicts.get(&Pair::new("mediaco", "main", "m1")).unwrap();
        assert_eq!(v.rejection, Some(want));
        assert_eq!(v.next, None, "BROKEN retests are off by default");
        assert!(v.reason.starts_with(&r.reason[..3]), "{v:?}");
    }
}

#[tokio::test]
async fn rate_limits_server_errors_and_malformed_answers_are_unknown() {
    let s = media(&[("mediaco", "main")]).await;
    // The 429 last: its rest holds the pair's next test.
    let cases = [
        rejected(503, "service unavailable"),
        rejected(404, "Not Found"),
        Step::json(200, json!({"object": "chat.completion", "choices": []})),
        rejected(429, "model m1 not found"),
    ];
    for step in cases {
        s.mock.on("/media/chat", [step]);
        let r = tested(&s, "mediaco/m1", None).await;
        assert_eq!(r.state, Some(State::Unknown), "{r:?}");
        let v = s.engine.verdicts.get(&Pair::new("mediaco", "main", "m1")).unwrap();
        assert_eq!((v.state, v.step), (State::Unknown, Some(0)), "{v:?}");
        assert!(v.next.is_some(), "an UNKNOWN is retested");
    }
    // Only a rate limit's rest holds a test back: the 503 and 404 rests didn't.
    assert_eq!(s.mock.received().len(), 4);
    let r = tested(&s, "mediaco/m1", None).await;
    assert!(r.skipped.as_deref().is_some_and(|w| w.starts_with("rate-limited until ")), "{r:?}");
    assert_eq!(s.mock.received().len(), 4, "no call");
}

#[tokio::test]
async fn a_hang_is_unknown_within_its_timeout() {
    let s = media(&[("mediaco", "main")]).await;
    s.mock.on("/media/chat", [Step::StallHeaders { hold: Duration::from_secs(60) }]);
    let started = Instant::now();
    let r = tested(&s, "mediaco/m1", None).await;
    assert!(started.elapsed() < Duration::from_secs(10), "SC-004: {:?}", started.elapsed());
    assert_eq!(r.state, Some(State::Unknown), "{r:?}");
    // The test's own limit, or the endpoint's header timeout if that is shorter.
    assert!(!r.reason.is_empty() && r.rejection.is_none(), "{r:?}");
    let rec = settled(&s, r.record.as_deref().unwrap()).await;
    assert_ne!(rec.outcome, Outcome::Succeeded);
}

#[tokio::test]
async fn a_test_calls_one_account_and_never_falls_back() {
    let s = media(&[("mediaco", "main"), ("mediaco", "spare")]).await;
    s.mock.on("/media/chat", [rejected(503, "upstream overloaded"), ok()]);
    let r = tested(&s, "mediaco/m1", Some("main")).await;
    assert_eq!(r.state, Some(State::Unknown), "{r:?}");
    assert_eq!(accounts_hit(&s), ["mediaco-main"], "no fallback to spare");
    assert!(s.engine.verdicts.get(&Pair::new("mediaco", "spare", "m1")).is_none());
}

#[tokio::test]
async fn a_rejected_key_marks_the_account_not_the_model() {
    let s = media(&[("mediaco", "main")]).await;
    s.mock.on("/media/chat", [rejected(401, "invalid api key for model m1")]);
    let r = tested(&s, "mediaco/m1", None).await;
    assert_eq!(r.state, Some(State::Unknown));
    assert!(r.reason.starts_with("the account was refused"), "{r:?}");
    assert!(s.engine.verdicts.get(&Pair::new("mediaco", "main", "m1")).is_none(), "FR-009");
}

#[tokio::test]
async fn a_disabled_account_is_skipped_without_a_call() {
    let s = setup_file(
        |m| vec![("mediaco", mediaco(m))],
        &format!(
            "schema = 1\n[[account]]\nprovider = \"mediaco\"\nname = \"off\"\nsecret = \"{SECRET}-mediaco-off\"\ndisabled = true\n"
        ),
        "",
    )
    .await;
    let r = tested(&s, "mediaco/m1", None).await;
    assert_eq!((r.state, r.skipped.as_deref()), (None, Some("account disabled")));
    assert!(s.mock.received().is_empty());
}

/// Spec 013 FR-025 and constitution VII: a test goes through the account's proxy, and a paused
/// proxy skips it with the reason instead of judging the model.
#[tokio::test]
async fn a_test_goes_through_the_accounts_proxy_and_a_paused_one_skips_it() {
    use nullrouter_engine::accounts::{self, Accounts};
    use nullrouter_engine::connection::fingerprints;
    use nullrouter_engine::testkit::MockProxy;

    let s = media(&[("mediaco", "main")]).await;
    let proxy = MockProxy::start().await;
    let home = s._dir.path();
    let proxies = format!("schema = 1\n\n[[proxy]]\nname = \"eu\"\nurl = \"http://{}\"\n", proxy.addr());
    nullrouter_engine::files::write_private(&home.join("proxies.toml"), &proxies).unwrap();
    let mut list = Accounts::load(&home.join(accounts::FILE)).unwrap();
    list.set_proxy("mediaco", "main", Some("eu".into())).unwrap();
    list.save().unwrap();
    s.engine.reload().await.unwrap();

    s.mock.on("/media/chat", [ok()]);
    let r = tested(&s, "mediaco/m1", None).await;
    assert_eq!(r.state, Some(State::Pass), "{r:?}");
    assert_eq!(proxy.carried(), 1, "the test call went through the account's proxy");

    let print = fingerprints(&s.engine.snapshot()).remove("eu").unwrap();
    s.engine.proxy_board.pause("eu", "connect to proxy failed", &print);
    let r = tested(&s, "mediaco/m1", None).await;
    assert_eq!((r.state, r.skipped.as_deref()), (None, Some("proxy eu paused")), "{r:?}");
    assert_eq!(proxy.carried(), 1, "nothing was sent");
    let v = s.engine.verdicts.get(&Pair::new("mediaco", "main", "m1")).unwrap();
    assert_eq!(v.state, State::Pass, "the skip leaves the verdict as it was");
}
