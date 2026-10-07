//! Model tests: one real, minimal call to a model on one account (spec 011 research R1, R2, R6,
//! R14).
//!
//! A test goes through [`Engine::reply`] like a client's request, pinned to its one account and
//! tagged as a test, so it is recorded, billed and paced like any call. The answer, or the
//! record's last failed attempt, gives the verdict, which the board keeps with the basis it
//! was reached on.

pub mod bodies;
pub mod retest;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use nullrouter_registry::schema::ModelType;
use nullrouter_registry::{ProviderEntity, Resolution};
use nullrouter_wire::codec::request;
use nullrouter_wire::codec::response::ForClient;
use nullrouter_wire::codec::types::JobStatus;
use nullrouter_wire::ir;
use reqwest::header::HeaderMap;
use serde::Serialize;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

use crate::attempt::{self, Answer, Media, MediaAnswer, Pin, TestTag, TextRequest};
use crate::keys::AgentId;
use crate::records::{self, AttemptOutcome, RequestRecord, TestMark};
use crate::state::{Engine, EngineState};
use crate::verdict::judge::{self, Failed};
use crate::verdict::{Basis, NO_ACCOUNT, Pair, Rejection, Source, State, Verdict};

/// The agent key a test's request runs under; its record keeps no agent.
pub const AGENT: &str = "nullrouter-test";

/// How often a video test polls its job.
const JOB_POLL: Duration = Duration::from_secs(5);

/// One pair a test will call, or why it won't.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Planned {
    #[serde(flatten)]
    pub pair: Pair,
    /// The model as the target names it; `pair.model` is the upstream id.
    #[serde(skip)]
    pub requested: String,
    #[serde(rename = "type")]
    pub ty: ModelType,
    /// Why no call will be made: the account is disabled, can't serve, or is resting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip: Option<String>,
}

/// What one pair's test found (data-model.md § Test run).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TestResult {
    #[serde(flatten)]
    pub pair: Pair,
    /// `None` when no call was made.
    pub state: Option<State>,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<Rejection>,
    pub ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    /// When the stored verdict is due a retest (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

impl TestResult {
    fn skipped(pair: &Pair, why: impl Into<String>) -> Self {
        let why = why.into();
        Self {
            pair: pair.clone(),
            state: None,
            reason: why.clone(),
            rejection: None,
            ms: 0,
            ttft_ms: None,
            record: None,
            skipped: Some(why),
            next: None,
        }
    }
}

/// A run's `tr_` id: the records of its calls carry it.
pub fn run_id() -> String {
    format!("tr_{}", records::new_id().trim_start_matches("rq_"))
}

/// The tests in flight, retests included, held under `[tests] concurrency` (research R10).
/// The limit is read at each entry, so a reload applies to calls not yet started.
#[derive(Default)]
pub struct Gate {
    busy: Mutex<u32>,
    freed: Notify,
}

pub struct Permit<'a> {
    gate: &'a Gate,
}

impl Gate {
    pub async fn enter(&self, limit: u32) -> Permit<'_> {
        loop {
            let freed = self.freed.notified();
            tokio::pin!(freed);
            freed.as_mut().enable();
            {
                let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
                if *busy < limit.max(1) {
                    *busy += 1;
                    return Permit { gate: self };
                }
            }
            freed.await;
        }
    }

    /// Tests in flight.
    pub fn busy(&self) -> u32 {
        *self.busy.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.gate.busy.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        self.gate.freed.notify_waiters();
    }
}

/// The pairs `target` reaches: `provider/model` on one or every account of its provider, a
/// unified model's members on each account that serves them, or, with no target, every pair
/// any loaded unified model reaches (FR-004). Each pair once.
pub fn expand(engine: &Engine, st: &EngineState, target: Option<&str>, account: Option<&str>) -> Result<Vec<Planned>, String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut add = |provider: &ProviderEntity, requested: &str, upstream: &str, ty: ModelType| -> Result<(), String> {
        for p in pairs(engine, st, provider, requested, upstream, ty, account)? {
            if seen.insert(p.pair.clone()) {
                out.push(p);
            }
        }
        Ok(())
    };
    let Some(target) = target else {
        for u in st.registry.unified_models() {
            for m in &u.members {
                let Ok(provider) = st.registry.provider(&m.provider) else { continue };
                let ty = model_type(st, provider, &m.requested).or(u.kind.and_then(ModelType::from_capability));
                // `all` covers every account; one named account narrows it.
                let _ = add(provider, &m.requested, &m.upstream_id, ty.unwrap_or(ModelType::Text));
            }
        }
        return Ok(out);
    };
    match st.registry.resolve_with(target, |p, m| st.live_models.has(p, m)).map_err(|e| e.to_string())? {
        Resolution::Direct { provider, requested, upstream_id, .. } => {
            let ty = model_type(st, provider, requested).unwrap_or(ModelType::Text);
            add(provider, requested, &upstream_id, ty)?;
        }
        Resolution::Unified(u) => {
            for m in &u.members {
                let provider = st.registry.provider(&m.provider).map_err(|e| e.to_string())?;
                let ty = model_type(st, provider, &m.requested).or(u.kind.and_then(ModelType::from_capability));
                add(provider, &m.requested, &m.upstream_id, ty.unwrap_or(ModelType::Text))?;
            }
        }
        // A combo test is one call through the combo, not a pair per member (research R14).
        Resolution::Combo(c) => return Err(format!("{} is a combo", c.name)),
    }
    if out.is_empty()
        && let Some(a) = account
    {
        return Err(format!("no account {a} serves {target}"));
    }
    Ok(out)
}

/// Billed calls per type, for `test.plan`'s confirmation.
pub fn calls(planned: &[Planned]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for p in planned.iter().filter(|p| p.skip.is_none()) {
        *out.entry(p.ty.to_string()).or_default() += 1;
    }
    out
}

/// A declared or live model's type; `None` for an untyped model.
fn model_type(st: &EngineState, provider: &ProviderEntity, requested: &str) -> Option<ModelType> {
    match st.registry.model(&provider.id, requested).ok().and_then(|m| m.kind) {
        Some(kind) => ModelType::from_capability(kind),
        None => st.live_models.find(&provider.id, requested).map(|m| m.ty),
    }
}

fn pairs(
    engine: &Engine,
    st: &EngineState,
    provider: &ProviderEntity,
    requested: &str,
    upstream: &str,
    ty: ModelType,
    account: Option<&str>,
) -> Result<Vec<Planned>, String> {
    let planned = |name: &str| {
        let pair = Pair::new(&provider.id, name, upstream);
        let skip = skip_reason(engine, st, &pair);
        Planned { pair, requested: requested.to_owned(), ty, skip }
    };
    if provider.auth.as_ref().is_some_and(|a| a.no_auth) {
        return Ok(match account {
            Some(a) if a != NO_ACCOUNT => Vec::new(),
            _ => vec![planned(NO_ACCOUNT)],
        });
    }
    let mut names: Vec<&str> = st
        .accounts
        .iter()
        .filter(|a| a.provider == provider.id)
        .map(|a| a.name.as_str())
        .filter(|n| account.is_none_or(|a| a == *n))
        .collect();
    names.sort_unstable();
    if names.is_empty() && account.is_none() {
        let pair = Pair::new(&provider.id, NO_ACCOUNT, upstream);
        let skip = Some(format!("no account for {}", provider.id));
        return Ok(vec![Planned { pair, requested: requested.to_owned(), ty, skip }]);
    }
    Ok(names.into_iter().map(planned).collect())
}

/// Why `pair` can't be called now, if it can't: no call is made (spec edge cases).
pub fn skip_reason(engine: &Engine, st: &EngineState, pair: &Pair) -> Option<String> {
    let name = match pair.account.as_str() {
        NO_ACCOUNT => "",
        n => {
            let Some(a) = st.accounts.get(&pair.provider, n) else {
                return Some(format!("no account {n} for {}", pair.provider));
            };
            if a.disabled {
                return Some("account disabled".into());
            }
            if let Some(w) = crate::accounts::out_of_service(a, &st.tokens) {
                return Some(w.to_string());
            }
            n
        }
    };
    let until = engine.cooldowns.rate_limited(&pair.provider, name, &pair.model)?;
    let left = until.saturating_duration_since(tokio::time::Instant::now());
    let at = crate::clock::rfc3339(SystemTime::now() + left);
    Some(format!("rate-limited until {} UTC", at.get(11..16).unwrap_or(&at)))
}

/// Runs `planned` under the gate, sending each result as it finishes. A cancelled `stop`
/// leaves calls not yet sent unmade; calls in flight finish and are kept.
pub async fn run(
    engine: &Arc<Engine>,
    planned: Vec<Planned>,
    source: Source,
    run: &str,
    stop: CancellationToken,
    tx: mpsc::Sender<TestResult>,
) {
    let mut tasks = tokio::task::JoinSet::new();
    for p in planned {
        let (engine, run, stop, tx) = (engine.clone(), run.to_owned(), stop.clone(), tx.clone());
        tasks.spawn(async move {
            if let Some(r) = run_pair(&engine, &p, source, &run, &stop).await {
                let _ = tx.send(r).await;
            }
        });
    }
    while tasks.join_next().await.is_some() {}
}

/// Tests one pair and stores its verdict. `None` when `stop` was cancelled before the call.
pub async fn run_pair(
    engine: &Arc<Engine>,
    p: &Planned,
    source: Source,
    run: &str,
    stop: &CancellationToken,
) -> Option<TestResult> {
    run_pair_at(engine, p, source, run, stop, None).await
}

/// [`run_pair`], with the verdict dated `at` instead of when the call ends: a simulated clock.
pub async fn run_pair_at(
    engine: &Arc<Engine>,
    p: &Planned,
    source: Source,
    run: &str,
    stop: &CancellationToken,
    at: Option<SystemTime>,
) -> Option<TestResult> {
    if let Some(why) = &p.skip {
        return Some(TestResult::skipped(&p.pair, why));
    }
    let st = engine.snapshot();
    let _permit = engine.test_gate.enter(st.registry.runtime().tests.concurrency).await;
    if stop.is_cancelled() {
        return None;
    }
    // The account may have changed while the call waited for the gate.
    let st = engine.snapshot();
    if let Some(why) = skip_reason(engine, &st, &p.pair) {
        return Some(TestResult::skipped(&p.pair, why));
    }
    let started = Instant::now();
    let id = records::new_id();
    let mut record = RequestRecord::new(id.clone(), crate::clock::now_rfc3339_millis(), bodies::STYLE);
    record.model_type = Some(p.ty);
    record.test = Some(TestMark { run: run.to_owned(), source });
    engine.records.insert(record);
    let cancel = CancellationToken::new();
    let tag = TestTag { run: run.to_owned(), source };
    let req = match request(&st, p, &id, cancel.clone(), tag) {
        Ok(r) => r,
        Err(e) => return Some(TestResult::skipped(&p.pair, e)),
    };
    let limit = st.registry.runtime().tests.timeout.for_kind(Some(p.ty.capability()));
    let called = match tokio::time::timeout(limit, call(engine, &st, req, p.ty)).await {
        Ok(c) => c,
        Err(_) => {
            cancel.cancel();
            Called::Failed { status: None, message: format!("timeout after {}", seconds(limit)) }
        }
    };
    let ms = started.elapsed().as_millis() as u64;
    let rec = engine.records.get(&id);
    let ttft_ms = rec.as_ref().and_then(|r| r.ttft_ms).map(|t| t as u64);
    let Ok(provider) = st.registry.provider(&p.pair.provider) else {
        return Some(TestResult::skipped(&p.pair, format!("provider {} isn't loaded", p.pair.provider)));
    };
    let judged = match called {
        Called::Pass => judge::Judged { state: State::Pass, rejection: None, reason: String::new() },
        Called::Malformed(why) => judge::Judged { state: State::Unknown, rejection: None, reason: judge::cut(&why) },
        Called::Failed { status, message } => {
            let attempt = rec.as_ref().and_then(last_failed);
            let f = match &attempt {
                Some((status, class, reason)) => Failed { status: *status, class: *class, message: reason },
                // No attempt reached the provider (it rested, say): no call was made.
                None if status.is_some() => return Some(TestResult::skipped(&p.pair, message)),
                None => Failed { status: None, class: records::ErrorClass::Timeout, message: &message },
            };
            if judge::account_fault(&f, provider) {
                let reason = format!("the account was refused, not the model: {}", judge::reason(&f));
                return Some(result(p, State::Unknown, reason, None, ms, ttft_ms, &id));
            }
            judge::judge(&f, provider)
        }
    };
    let now = at.unwrap_or_else(SystemTime::now);
    let next = store(engine, &st, &p.pair, source, &judged, &id, now);
    let mut r = result(p, judged.state, judged.reason, judged.rejection, ms, ttft_ms, &id);
    r.next = next.map(crate::clock::rfc3339);
    Some(r)
}

fn result(
    p: &Planned,
    state: State,
    reason: String,
    rejection: Option<Rejection>,
    ms: u64,
    ttft_ms: Option<u64>,
    record: &str,
) -> TestResult {
    TestResult {
        pair: p.pair.clone(),
        state: Some(state),
        reason,
        rejection,
        ms,
        ttft_ms,
        record: Some(record.to_owned()),
        skipped: None,
        next: None,
    }
}

/// `30 s`, `5 min`.
fn seconds(d: Duration) -> String {
    match d.as_secs() {
        s if s >= 60 && s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

fn last_failed(r: &RequestRecord) -> Option<(Option<u16>, records::ErrorClass, String)> {
    r.attempts.iter().rev().find_map(|a| match &a.outcome {
        Some(AttemptOutcome::Failed { status, class, reason }) => Some((*status, *class, reason.clone())),
        _ => None,
    })
}

/// The test's request in the Chat Completions style, pinned to the pair's account.
fn request(
    st: &EngineState,
    p: &Planned,
    id: &str,
    cancel: CancellationToken,
    tag: TestTag,
) -> Result<TextRequest, String> {
    let style = st.style(bodies::STYLE).ok_or_else(|| format!("style {} isn't loaded", bodies::STYLE))?.clone();
    let target = format!("{}/{}", p.pair.provider, p.requested);
    let mut body = bodies::body(p.ty);
    body["model"] = target.clone().into();
    let (ir, media) = match p.ty {
        ModelType::Text => (request::decode(&style, &body).map_err(|e| e.to_string())?, None),
        ty => {
            let codec = style.type_codec(ty).map_err(|e| e.to_string())?.variant(None).into_owned();
            let input = codec.decode_request(&body).map_err(|e| e.to_string())?;
            let job = ty == ModelType::Video;
            (ir::Request::default(), Some(Media { ty, codec, variant: None, input, voice: None, job }))
        }
    };
    Ok(TextRequest {
        id: id.to_owned(),
        arrived: Instant::now(),
        client: style,
        body,
        ir,
        headers: HeaderMap::new(),
        agent: AgentId::new(AGENT.to_owned(), None),
        target,
        stream: false,
        cancel,
        media,
        count: false,
        pin: Some(Pin { provider: p.pair.provider.clone(), account: p.pair.account.clone() }),
        test: Some(tag),
    })
}

/// How the call ended, before judging.
enum Called {
    Pass,
    /// A success whose body doesn't have the answer (R2's PASS check failed).
    Malformed(String),
    /// No answer: the record's attempts say why; `status` and `message` are the engine's.
    Failed { status: Option<u16>, message: String },
}

impl Called {
    fn checked(r: Result<(), String>) -> Self {
        r.map_or_else(Called::Malformed, |()| Called::Pass)
    }
}

async fn call(engine: &Arc<Engine>, st: &Arc<EngineState>, req: TextRequest, ty: ModelType) -> Called {
    let (client, body) = (req.client.clone(), req.body.clone());
    let reply = match engine.reply(st.clone(), req).await {
        Ok(r) => r,
        Err(f) => return Called::Failed { status: Some(f.status), message: f.message },
    };
    match reply.answer {
        Answer::Whole { answer, .. } => match *answer {
            ForClient::AsReceived { read: Some(r) } | ForClient::Rebuilt { read: r, .. } => {
                Called::checked(bodies::check_text(&r))
            }
            ForClient::AsReceived { read: None } => {
                Called::Malformed("malformed answer: the body doesn't have the style's response shape".into())
            }
        },
        Answer::Events { rx, .. } => match attempt::collect(&client, &body, rx).await {
            Ok(r) => Called::checked(bodies::check_text(&r)),
            Err(e) => Called::Failed { status: None, message: e.message },
        },
        Answer::Media(MediaAnswer::Value(v)) => Called::checked(bodies::check_value(ty, &v)),
        Answer::Media(MediaAnswer::Bytes { rx, .. }) => first_bytes(rx, ty).await,
        Answer::Media(MediaAnswer::Job { id, status, .. }) => follow(engine, st, &id, status).await,
        Answer::Count { .. } => Called::Malformed("malformed answer: a token count".into()),
    }
}

/// PASS once the first non-empty chunk arrives.
async fn first_bytes(mut rx: mpsc::Receiver<Result<bytes::Bytes, String>>, ty: ModelType) -> Called {
    while let Some(chunk) = rx.recv().await {
        match chunk {
            Ok(b) if b.is_empty() => continue,
            Ok(_) => return Called::Pass,
            Err(e) => return Called::Failed { status: None, message: e },
        }
    }
    Called::Malformed(format!("malformed answer: no {} in the response", if ty == ModelType::Video { "video" } else { "audio" }))
}

/// Polls a video job to `completed`, then reads its first content bytes. The test's timeout
/// bounds the whole wait.
async fn follow(engine: &Arc<Engine>, st: &EngineState, id: &str, mut status: JobStatus) -> Called {
    loop {
        match status {
            JobStatus::Completed => break,
            JobStatus::Failed => return Called::Failed { status: None, message: "the video job failed".into() },
            _ => tokio::time::sleep(JOB_POLL).await,
        }
        status = match engine.job_get(st, id, AGENT, bodies::STYLE).await {
            Ok((_, _, s)) => s,
            Err(f) => return Called::Failed { status: None, message: f.message },
        };
    }
    match engine.job_content(st, id, AGENT, bodies::STYLE).await {
        Ok((_, rx)) => first_bytes(rx, ModelType::Video).await,
        Err(f) => Called::Failed { status: None, message: f.message },
    }
}

/// Stores `j` for `pair` at `now` with its retest step, returning when the retest is due
/// (research R6, R8): an UNKNOWN's step advances only on a retest of an UNKNOWN; any other
/// UNKNOWN starts at step 0.
fn store(
    engine: &Engine,
    st: &EngineState,
    pair: &Pair,
    source: Source,
    j: &judge::Judged,
    record: &str,
    now: SystemTime,
) -> Option<SystemTime> {
    let step = (j.state == State::Unknown).then(|| match engine.verdicts.get(pair) {
        Some(v) if v.state == State::Unknown && source == Source::Retest => v.step.map_or(0, |s| s + 1),
        _ => 0,
    });
    let mut v = Verdict {
        state: j.state,
        reason: j.reason.clone(),
        rejection: j.rejection.clone(),
        source,
        at: now,
        record: Some(record.to_owned()),
        step,
        next: None,
        basis: basis(engine, st, pair),
        note: None,
    };
    v.next = retest::due(&v, &st.registry.runtime().tests);
    let next = v.next;
    engine.verdicts.set(pair.clone(), v);
    next
}

/// What `pair`'s verdict rests on: the account's secret or sign-in, and its plugin. A change in
/// any returns the pair to untested.
pub fn basis(engine: &Engine, st: &EngineState, pair: &Pair) -> Basis {
    use sha2::{Digest, Sha256};
    let account = st.accounts.get(&pair.provider, &pair.account);
    let secret = account.filter(|a| !a.is_signin()).and_then(|a| a.secret.as_ref()).and_then(|s| {
        let install = engine.install_id().ok()?;
        Some(s.with_exposed(|k| {
            let mut h = Sha256::new();
            h.update(install.as_bytes());
            h.update([0u8]);
            h.update(k.as_bytes());
            format!("sha256:{:x}", h.finalize())
        }))
    });
    let signed_in_at = account
        .filter(|a| a.is_signin())
        .and_then(|a| st.tokens.get(&a.provider, &a.name))
        .map(|t| crate::clock::rfc3339(t.entry.signed_in_at));
    let plugin = st.registry.plugin_digest(&pair.provider).unwrap_or_default().to_owned();
    Basis { secret, signed_in_at, plugin }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    #[tokio::test]
    async fn the_gate_holds_the_limit() {
        let gate = Arc::new(Gate::default());
        let a = gate.enter(2).await;
        let _b = gate.enter(2).await;
        assert_eq!(gate.busy(), 2);
        let g = gate.clone();
        let third = tokio::spawn(async move {
            let _p = g.enter(2).await;
        });
        tokio::task::yield_now().await;
        assert!(!third.is_finished());
        drop(a);
        third.await.unwrap();
        assert_eq!(gate.busy(), 1);
    }
}
