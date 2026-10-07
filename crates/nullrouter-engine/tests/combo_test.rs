//! Spec 011 T048, US6 scenarios 1–5, SC-007, FR-030: a combo test is one call through the combo
//! as a client would make it. Its result names the member that answered and nests each member
//! it tried under its combo; a PASS or a definitive rejection updates its pair, any other failure
//! shows only in the output. The combo's own result survives a restart, is retested while
//! UNKNOWN, and is cleared when the combo's definition changes.

mod common;

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use common::*;
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::Step;
use nullrouter_engine::tests::combo::{self, ComboResult, MemberKind, Tried};
use nullrouter_engine::tests::retest::Retester;
use nullrouter_engine::tests;
use nullrouter_engine::verdict::{Pair, Source, State, Verdict};
use nullrouter_registry::OperatorHome;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const NO_RETRY: &str = "retry = { 429 = { retries = 0 }, 500 = { retries = 0 }, 502 = { retries = 0 }, 503 = { retries = 0 }, 504 = { retries = 0 } }";
const SECOND: Duration = Duration::from_secs(1);

/// Providers `alpha`, `beta` and `gamma`, one account `a` each; unified models `ua`, `ub`, `uc`
/// over them; combos from `combos`.
async fn fleet(combos: &str) -> Setup {
    let config = format!("{}{combos}", unified_models());
    setup(
        |m| {
            vec![
                ("alpha", chat_plugin(m, "alpha", NO_RETRY)),
                ("beta", chat_plugin(m, "beta", NO_RETRY)),
                ("gamma", chat_plugin(m, "gamma", NO_RETRY)),
            ]
        },
        &[("alpha", "a"), ("beta", "a"), ("gamma", "a")],
        &config,
    )
    .await
}

fn unified_models() -> String {
    ["alpha", "beta", "gamma"]
        .iter()
        .map(|p| {
            let name = format!("u{}", &p[..1]);
            format!("[[unified_model]]\nname = \"{name}\"\nmembers = [{{ provider = \"{p}\", model = \"m1\" }}]\n")
        })
        .collect()
}

const CODER: &str = "[[combo]]\nname = \"coder\"\nmembers = [\"ua\", \"ub\"]\n";

/// `coder` → `ua`, `chain`; `chain` → `ub`, `deep`; `deep` → `uc`.
const NESTED: &str = "[[combo]]\nname = \"coder\"\nmembers = [\"ua\", \"chain\"]\n\
                      [[combo]]\nname = \"chain\"\nmembers = [\"ub\", \"deep\"]\n\
                      [[combo]]\nname = \"deep\"\nmembers = [\"uc\"]\n";

fn m1(provider: &str) -> Pair {
    Pair::new(provider, "a", "m1")
}

fn not_found() -> Step {
    Step::json(404, json!({"error": {"message": "The model 'm1' does not exist"}}))
}

/// `state` for `pair`, set by a test, on the basis the pair has now.
fn verdict(s: &Setup, pair: &Pair, state: State) -> Verdict {
    let st = s.engine.snapshot();
    Verdict {
        state,
        reason: "set before the combo test".into(),
        rejection: None,
        source: Source::Test,
        at: SystemTime::now(),
        record: None,
        step: None,
        next: None,
        basis: tests::basis(&s.engine, &st, pair),
        note: None,
    }
}

async fn test_combo(engine: &Arc<Engine>, source: Source, at: Option<SystemTime>) -> ComboResult {
    combo::run_combo_at(engine, "coder", source, "tr_c", &CancellationToken::new(), at).await.unwrap().unwrap()
}

/// `(member, kind, state)` of each member reached, nested ones after their combo.
fn shape(tried: &[Tried]) -> Vec<(String, MemberKind, State)> {
    let mut out = Vec::new();
    for t in tried {
        out.push((t.member.clone(), t.kind, t.state));
        out.extend(shape(&t.tried));
    }
    out
}

fn row(member: &str, kind: MemberKind, state: State) -> (String, MemberKind, State) {
    (member.to_owned(), kind, state)
}

#[tokio::test]
async fn the_first_member_answers_and_no_other_is_called() {
    let s = fleet(CODER).await;
    s.mock.on("/alpha", [ok()]);
    let r = test_combo(&s.engine, Source::Test, None).await;
    assert_eq!((r.state, r.answered_by.as_deref()), (State::Pass, Some("ua")), "{r:#?}");
    assert_eq!(paths(&s), ["/alpha/chat/completions"], "scenario 1: no other member is called");
    assert_eq!(shape(&r.tried), [row("ua", MemberKind::Unified, State::Pass)]);

    let v = s.engine.verdicts.get(&m1("alpha")).unwrap();
    assert_eq!((v.state, v.source), (State::Pass, Source::ComboTest));
    assert!(s.engine.verdicts.get(&m1("beta")).is_none());

    let rec = settled(&s, r.record.as_deref().unwrap()).await;
    assert_eq!(rec.combo.as_deref(), Some("coder"));
    assert_eq!(rec.test.map(|t| t.run), Some("tr_c".to_owned()), "recorded as a test");
    let kept = s.engine.verdicts.combo("coder").unwrap();
    assert_eq!((kept.state, kept.answered_by.as_deref(), kept.next), (State::Pass, Some("ua"), None));
}

#[tokio::test]
async fn a_failure_that_isnt_definitive_gives_unknown_and_changes_no_pair() {
    let s = fleet(CODER).await;
    s.engine.verdicts.set(m1("alpha"), verdict(&s, &m1("alpha"), State::Pass));
    s.mock.respond(|_| err(503));
    let r = test_combo(&s.engine, Source::Test, None).await;
    assert_eq!(r.state, State::Unknown, "{r:#?}");
    assert!(r.answered_by.is_none());
    assert!(r.reason.contains("503"), "{}", r.reason);
    assert_eq!(
        shape(&r.tried),
        [row("ua", MemberKind::Unified, State::Unknown), row("ub", MemberKind::Unified, State::Unknown)]
    );

    // Scenario 4: a 503 keeps the pair's verdict and starts no retest of it.
    let alpha = s.engine.verdicts.get(&m1("alpha")).unwrap();
    assert_eq!((alpha.state, alpha.source, alpha.next), (State::Pass, Source::Test, None));
    assert!(s.engine.verdicts.get(&m1("beta")).is_none());
    let kept = s.engine.verdicts.combo("coder").unwrap();
    assert_eq!((kept.state, kept.step), (State::Unknown, Some(0)));
    assert!(kept.next.is_some(), "an UNKNOWN combo is retested (FR-030)");
}

#[tokio::test]
async fn every_member_broken_or_rejected_gives_broken() {
    let s = fleet(CODER).await;
    s.engine.verdicts.set(m1("alpha"), verdict(&s, &m1("alpha"), State::Broken));
    s.mock.on("/beta", [not_found()]);
    let r = test_combo(&s.engine, Source::Test, None).await;
    assert_eq!(r.state, State::Broken, "{r:#?}");
    assert_eq!(paths(&s), ["/beta/chat/completions"], "a BROKEN pair is skipped without a call");
    assert_eq!(r.tried[0].reason, "skipped: BROKEN on every account");
    assert_eq!(r.tried[1].attempts[0].state, Some(State::Broken));

    // Scenario 4: the rejection updates its pair; the skipped pair keeps its own verdict.
    let beta = s.engine.verdicts.get(&m1("beta")).unwrap();
    assert_eq!((beta.state, beta.source), (State::Broken, Source::ComboTest));
    assert_eq!(s.engine.verdicts.get(&m1("alpha")).unwrap().source, Source::Test);
    assert_eq!(s.engine.verdicts.combo("coder").unwrap().next, None, "a BROKEN combo isn't retested");
}

/// The spec's independent test and SC-007: three levels, the first leaf BROKEN, the second
/// UNKNOWN, the third answers.
#[tokio::test]
async fn a_nested_combo_shows_each_member_under_its_combo() {
    let s = fleet(NESTED).await;
    s.engine.verdicts.set(m1("alpha"), verdict(&s, &m1("alpha"), State::Broken));
    s.mock.on("/beta", [err(503)]);
    s.mock.on("/gamma", [ok()]);
    let r = test_combo(&s.engine, Source::Test, None).await;
    assert_eq!((r.state, r.answered_by.as_deref()), (State::Pass, Some("uc")), "{r:#?}");
    assert_eq!(
        shape(&r.tried),
        [
            row("ua", MemberKind::Unified, State::Broken),
            row("chain", MemberKind::Combo, State::Pass),
            row("ub", MemberKind::Unified, State::Unknown),
            row("deep", MemberKind::Combo, State::Pass),
            row("uc", MemberKind::Unified, State::Pass),
        ]
    );
    let ub = &r.tried[1].tried[0];
    assert!(ub.reason.contains("503"), "{ub:#?}");
    assert!(ub.attempts[0].skipped.is_none(), "a call was made");
    assert!(s.engine.verdicts.get(&m1("beta")).is_none(), "the 503 isn't saved");
    let gamma = s.engine.verdicts.get(&m1("gamma")).unwrap();
    assert_eq!((gamma.state, gamma.source), (State::Pass, Source::ComboTest));

    // The JSON nests the same way (data-model.md § ComboResult).
    let j = serde_json::to_value(&r).unwrap();
    assert_eq!(j["tried"][1]["kind"], "combo");
    assert_eq!(j["tried"][1]["tried"][1]["tried"][0]["member"], "uc");
    assert_eq!(j["tried"][1]["tried"][1]["tried"][0]["attempts"][0]["provider"], "gamma");
}

/// Scenario 5 on a simulated clock: an UNKNOWN result survives a restart, is retested on the
/// schedule until PASS, and is cleared when the combo's definition changes.
#[tokio::test]
async fn an_unknown_combo_is_kept_retested_and_cleared_by_a_new_definition() {
    let s = fleet(CODER).await;
    s.mock.respond(|_| err(503));
    // Whole seconds, as the file keeps them.
    let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(since.as_secs());
    let r = test_combo(&s.engine, Source::Test, Some(t0)).await;
    assert_eq!(r.state, State::Unknown);

    let engine = s.engine.clone();
    tokio::task::spawn_blocking(move || engine.journal.flush_blocking()).await.unwrap();
    let home = s._dir.path().to_owned();
    let open = home.clone();
    let engine = tokio::task::spawn_blocking(move || Engine::open_parity(OperatorHome::new(open)).unwrap().0)
        .await
        .unwrap();
    let engine = Arc::new(engine);
    let kept = engine.verdicts.combo("coder").expect("the result survives a restart");
    assert_eq!((kept.state, kept.next), (State::Unknown, Some(t0 + 60 * SECOND)));

    let mut r = Retester::default();
    assert!(r.due_combos(&engine, t0 + 59 * SECOND).0.is_empty());
    assert_eq!(r.due_combos(&engine, t0 + 60 * SECOND).0, ["coder"]);
    assert!(r.due_combos(&engine, t0 + 61 * SECOND).0.is_empty(), "running");
    let t1 = t0 + 60 * SECOND;
    assert_eq!(test_combo(&engine, Source::Retest, Some(t1)).await.state, State::Unknown);
    r.combo_done("coder");
    let kept = engine.verdicts.combo("coder").unwrap();
    assert_eq!((kept.step, kept.next), (Some(1), Some(t1 + 300 * SECOND)), "the next step of the schedule");

    s.mock.respond(|_| ok());
    let t2 = t1 + 300 * SECOND;
    assert_eq!(r.due_combos(&engine, t2).0, ["coder"]);
    assert_eq!(test_combo(&engine, Source::Retest, Some(t2)).await.state, State::Pass);
    r.combo_done("coder");
    assert_eq!(engine.verdicts.combo("coder").unwrap().next, None, "settled");
    assert!(r.due_combos(&engine, t2 + 86_400 * SECOND).0.is_empty());

    // A new definition: the result no longer holds.
    let swapped = CODER.replace("[\"ua\", \"ub\"]", "[\"ub\", \"ua\"]");
    let config = format!("allow_private_endpoints = true\n{}{swapped}", unified_models());
    std::fs::write(home.join("config.toml"), config).unwrap();
    engine.reload().await.unwrap();
    assert!(engine.verdicts.combo("coder").is_none(), "cleared by the changed definition");
}
