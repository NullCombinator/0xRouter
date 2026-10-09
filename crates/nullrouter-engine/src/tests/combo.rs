//! Combo tests (spec 011 research R14): one call through a combo, as a client would make it.
//!
//! The call is tagged as a test but not pinned, so the combo walks its members as it would for
//! a client. Each attempt of the walk is then judged from the record: a PASS or a definitive
//! rejection updates its pair, with source `combo_test`; any other failure shows only in the
//! output (clarify Q3). The combo's own result is kept on the board under its name and retested
//! while UNKNOWN (FR-030); it never steers routing.

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use nullrouter_registry::Combo;
use nullrouter_registry::schema::ModelType;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::{Called, TestResult};
use crate::attempt::TestTag;
use crate::records::{Attempt, AttemptOutcome, ErrorClass};
use crate::state::{Engine, EngineState};
use crate::verdict::judge::{self, Failed};
use crate::verdict::{self, ComboVerdict, NO_ACCOUNT, Pair, Source, State};

/// What a combo test found (data-model.md § Test run).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComboResult {
    pub combo: String,
    pub state: State,
    /// The unified model that answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
    pub reason: String,
    pub ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// When the result is due a retest (RFC 3339): UNKNOWN only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// The members the walk reached, in order, nested combos with their own.
    pub tried: Vec<Tried>,
}

/// One member of a combo the walk reached.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tried {
    pub member: String,
    pub kind: MemberKind,
    pub state: State,
    pub reason: String,
    /// A unified model's attempts, skips included.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<TestResult>,
    /// A nested combo's members.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tried: Vec<Tried>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberKind {
    Unified,
    Combo,
}

/// The type of call a combo test makes: the combo's kind, text when untyped.
pub fn kind(combo: &Combo) -> ModelType {
    combo.kind.and_then(ModelType::from_capability).unwrap_or(ModelType::Text)
}

/// Tests combo `name` and keeps its result. `Ok(None)` when `stop` was cancelled before the call;
/// the error is an unknown combo or a request that couldn't be built.
pub async fn run_combo(
    engine: &Arc<Engine>,
    name: &str,
    source: Source,
    run: &str,
    stop: &CancellationToken,
) -> Result<Option<ComboResult>, String> {
    run_combo_at(engine, name, source, run, stop, None).await
}

/// [`run_combo`], with the results dated `at` instead of when the call ends: a simulated clock.
pub async fn run_combo_at(
    engine: &Arc<Engine>,
    name: &str,
    source: Source,
    run: &str,
    stop: &CancellationToken,
    at: Option<SystemTime>,
) -> Result<Option<ComboResult>, String> {
    let st = engine.snapshot();
    st.registry.combo(name).ok_or_else(|| format!("no combo {name}"))?;
    let _permit = engine.test_gate.enter(st.registry.runtime().tests.concurrency).await;
    if stop.is_cancelled() {
        return Ok(None);
    }
    // The combo may have changed while the call waited for the gate.
    let st = engine.snapshot();
    let combo = st.registry.combo(name).ok_or_else(|| format!("no combo {name}"))?;
    let ty = kind(combo);
    let started = Instant::now();
    let id = super::open_record(engine, ty, run, source);
    let cancel = CancellationToken::new();
    let tag = TestTag { run: run.to_owned(), source };
    let req = super::request(&st, ty, combo.name.clone(), None, &id, cancel.clone(), tag)?;
    let called = super::within(engine, &st, req, ty, cancel).await;
    let ms = started.elapsed().as_millis() as u64;
    let now = at.unwrap_or_else(SystemTime::now);
    let attempts = engine.records.get(&id).map(|r| r.attempts).unwrap_or_default();
    let judged: Vec<(Option<String>, TestResult)> = attempts
        .iter()
        .map(|a| (a.member.clone(), attempt(engine, &st, a, &id, &called, now)))
        .collect();

    let answered_by = match called {
        Called::Pass => judged.iter().rev().find(|(_, r)| r.state == Some(State::Pass)).and_then(|(m, _)| {
            m.as_deref().map(|m| m.rsplit(" › ").next().unwrap_or(m).to_owned())
        }),
        _ => None,
    };
    let state = match &answered_by {
        Some(_) => State::Pass,
        None => settle(judged.iter().map(|(_, r)| r.state)),
    };
    let reason = match state {
        State::Pass => String::new(),
        State::Broken => "every member is BROKEN or was rejected".to_owned(),
        State::Unknown => judged
            .iter()
            .rev()
            .map(|(_, r)| r)
            .find(|r| r.state != Some(State::Broken))
            .map(|r| r.reason.clone())
            .or_else(|| match &called {
                Called::Failed { message, .. } | Called::Malformed(message) => Some(judge::cut(message)),
                Called::Pass => None,
            })
            .unwrap_or_default(),
    };
    let tried = level(&st, combo, &combo.name, &judged);

    let before = engine.verdicts.combo(name).map(|v| (v.state, v.step));
    let mut v = ComboVerdict {
        state,
        answered_by: answered_by.clone(),
        reason: reason.clone(),
        at: now,
        record: Some(id.clone()),
        step: super::step(before, state, source),
        next: None,
        definition: verdict::definition(combo),
    };
    v.next = super::retest::combo_due(&v, &st.registry.runtime().tests);
    let next = v.next.map(crate::clock::rfc3339);
    engine.verdicts.set_combo(name, v);
    Ok(Some(ComboResult { combo: combo.name.clone(), state, answered_by, reason, ms, record: Some(id), next, tried }))
}

/// One attempt of the walk as a test result. A call that answered or was definitively rejected
/// sets its pair's verdict (source `combo_test`); any other failure is shown, not kept.
fn attempt(
    engine: &Engine,
    st: &EngineState,
    a: &Attempt,
    record: &str,
    called: &Called,
    now: SystemTime,
) -> TestResult {
    let pair = Pair::new(&a.provider, a.account.as_deref().unwrap_or(NO_ACCOUNT), &a.model);
    let ms = a.ended.map_or(0.0, |e| e - a.started).max(0.0) as u64;
    let mut r = TestResult::skipped(&pair, "");
    (r.skipped, r.ms, r.record) = (None, ms, Some(record.to_owned()));
    let judged = match &a.outcome {
        Some(AttemptOutcome::Skipped { reason, class }) => {
            // A pair skipped as BROKEN stays BROKEN; any other skip made no call.
            r.state = (*class == Some(ErrorClass::Broken)).then_some(State::Broken);
            (r.reason, r.skipped) = (reason.clone(), Some(reason.clone()));
            return r;
        }
        Some(AttemptOutcome::Ok) => match called {
            Called::Pass => judge::Judged { state: State::Pass, rejection: None, reason: String::new() },
            Called::Malformed(why) | Called::Failed { message: why, .. } => {
                judge::Judged { state: State::Unknown, rejection: None, reason: judge::cut(why) }
            }
        },
        Some(AttemptOutcome::Failed { status, class, reason }) => {
            let f = Failed { status: *status, class: *class, message: reason };
            match st.registry.provider(&a.provider) {
                Ok(provider) if judge::account_fault(&f, provider) => judge::Judged {
                    state: State::Unknown,
                    rejection: None,
                    reason: format!("the account was refused, not the model: {}", judge::reason(&f)),
                },
                Ok(provider) => judge::judge(&f, provider),
                Err(_) => judge::Judged { state: State::Unknown, rejection: None, reason: judge::reason(&f) },
            }
        }
        // Cut off by the test's timeout, or never finished.
        Some(AttemptOutcome::Cancelled) | None => {
            let why = match called {
                Called::Failed { message, .. } => judge::cut(message),
                _ => "the attempt was cancelled".to_owned(),
            };
            judge::Judged { state: State::Unknown, rejection: None, reason: why }
        }
    };
    if judged.state != State::Unknown {
        r.next = super::store(engine, st, &pair, Source::ComboTest, &judged, record, now).map(crate::clock::rfc3339);
    }
    (r.state, r.reason, r.rejection) = (Some(judged.state), judged.reason, judged.rejection);
    r
}

/// PASS when any answered, BROKEN when there were some and every one is BROKEN, else UNKNOWN.
fn settle(states: impl IntoIterator<Item = Option<State>>) -> State {
    let (mut any, mut all_broken) = (false, true);
    for s in states {
        match s {
            Some(State::Pass) => return State::Pass,
            Some(State::Broken) => {}
            _ => all_broken = false,
        }
        any = true;
    }
    if any && all_broken { State::Broken } else { State::Unknown }
}

/// The members of `combo` (reached as `path`) that the walk reached, each with its attempts or,
/// for a nested combo, its own members. A unified model reached twice was tried once, under its
/// first path, so it shows only there.
fn level(st: &EngineState, combo: &Combo, path: &str, judged: &[(Option<String>, TestResult)]) -> Vec<Tried> {
    let mut out = Vec::new();
    for name in &combo.members {
        let path = format!("{path} › {name}");
        let t = match st.registry.combo(name) {
            Some(nested) => {
                let tried = level(st, nested, &path, judged);
                if tried.is_empty() {
                    continue;
                }
                let state = settle(tried.iter().map(|t| Some(t.state)));
                let reason = match state {
                    State::Pass => String::new(),
                    _ => tried.last().map(|t| t.reason.clone()).unwrap_or_default(),
                };
                Tried { member: name.clone(), kind: MemberKind::Combo, state, reason, attempts: Vec::new(), tried }
            }
            None => {
                let mine = judged.iter().filter(|(m, _)| m.as_deref() == Some(path.as_str()));
                let attempts: Vec<TestResult> = mine.map(|(_, r)| r.clone()).collect();
                if attempts.is_empty() {
                    continue;
                }
                let state = settle(attempts.iter().map(|r| r.state));
                let reason = match state {
                    State::Pass => String::new(),
                    State::Broken if attempts.iter().all(|r| r.skipped.is_some()) => {
                        "skipped: BROKEN on every account".to_owned()
                    }
                    _ => attempts.last().map(|r| r.reason.clone()).unwrap_or_default(),
                };
                Tried { member: name.clone(), kind: MemberKind::Unified, state, reason, attempts, tried: Vec::new() }
            }
        };
        out.push(t);
    }
    out
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    #[test]
    fn a_pass_wins_and_broken_needs_every_one() {
        assert_eq!(settle([Some(State::Broken), Some(State::Pass)]), State::Pass);
        assert_eq!(settle([Some(State::Broken), Some(State::Broken)]), State::Broken);
        assert_eq!(settle([Some(State::Broken), None]), State::Unknown);
        assert_eq!(settle([Some(State::Broken), Some(State::Unknown)]), State::Unknown);
        assert_eq!(settle([]), State::Unknown);
    }
}
