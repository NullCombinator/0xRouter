//! Retests (spec 011 research R8): an UNKNOWN verdict is tested again on the `[tests] retest`
//! schedule until it settles, and a test's BROKEN every `broken_retest` when that is on. The
//! task runs beside maintenance, never in its queue, so a long retest can't hold back a token
//! refresh; its only limit is the test gate it shares with operator tests.
//!
//! What is due is worked out from the board and the settings at each wake, so a changed
//! schedule applies to the next retest without a restart. A pair whose account can't serve, or
//! couldn't take cold work, waits ([`waiting`]) and keeps its verdict. Two tests of one pair at
//! once (an operator's and a retest) both run; the one that finishes last is kept.
//!
//! An UNKNOWN combo result is retested the same way, one call through the combo each time,
//! held while no account of the first unified model it would try can serve (FR-030).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nullrouter_registry::Combo;
use nullrouter_registry::schema::{ModelType, TestSettings};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use super::Planned;
use crate::state::{Engine, EngineState};
use crate::verdict::{ComboVerdict, NO_ACCOUNT, Pair, Source, State, Verdict};

/// After a restart, the n-th overdue retest waits n × this.
pub const SPREAD: Duration = Duration::from_secs(10);
/// The longest sleep without a due retest. It is under the shortest step a schedule may have
/// (30 s), so a verdict set while the task sleeps is retested on time.
pub const IDLE_WAKE: Duration = Duration::from_secs(30);

/// When `v` is next due a retest under `tests`, if ever: an UNKNOWN at `at + retest[step]`,
/// the last step repeating; a test's BROKEN every `broken_retest` when on; an operator's
/// BROKEN and a PASS never.
pub fn due(v: &Verdict, tests: &TestSettings) -> Option<SystemTime> {
    match v.state {
        State::Unknown => unknown_due(v.at, v.step, tests),
        State::Broken if v.source != Source::Operator => tests.broken_retest.map(|w| v.at + w),
        _ => None,
    }
}

/// When a combo result is next due a retest: an UNKNOWN on the `retest` schedule, as a pair's.
pub fn combo_due(v: &ComboVerdict, tests: &TestSettings) -> Option<SystemTime> {
    (v.state == State::Unknown).then(|| unknown_due(v.at, v.step, tests)).flatten()
}

fn unknown_due(at: SystemTime, step: Option<u32>, tests: &TestSettings) -> Option<SystemTime> {
    let step = step.unwrap_or(0) as usize;
    let wait = tests.retest.get(step).or(tests.retest.last())?;
    Some(at + *wait)
}

/// Why `pair`'s retest waits, if it does: its account can't serve (disabled, needs sign-in,
/// refreshing, rate-limited) or a quota window is at its reserve floor (clarify Q4). Shown in
/// the verdict list, never stored.
pub fn waiting(engine: &Engine, st: &EngineState, pair: &Pair, now: SystemTime) -> Option<String> {
    if let Some(why) = super::skip_reason(engine, st, pair) {
        return Some(why);
    }
    let a = st.accounts.get(&pair.provider, &pair.account)?;
    let provider = st.registry.provider(&pair.provider).ok()?;
    crate::route::account_quota(engine, provider, a, now)
        .at_floor()
        .then(|| "a quota window is at its reserve floor".to_owned())
}

/// Why `combo`'s retest waits, if it does: no account of the first unified model it would try
/// can serve now. Shown in the verdict list, never stored.
pub fn combo_waiting(engine: &Engine, st: &EngineState, combo: &Combo) -> Option<String> {
    let (_, first) = st.registry.combo_walk(combo).next()?;
    let serves = first.members.iter().any(|m| {
        let Ok(provider) = st.registry.provider(&m.provider) else { return false };
        if provider.auth.as_ref().is_some_and(|a| a.no_auth) {
            let pair = Pair::new(&provider.id, NO_ACCOUNT, &m.upstream_id);
            return super::skip_reason(engine, st, &pair).is_none();
        }
        st.accounts.iter().filter(|a| a.provider == provider.id).any(|a| {
            let pair = Pair::new(&provider.id, &a.name, &m.upstream_id);
            super::skip_reason(engine, st, &pair).is_none()
        })
    });
    (!serves).then(|| format!("no account of {} can serve", first.name))
}

/// `pair` as a test would call it; `None` when its provider is gone.
fn planned(st: &EngineState, pair: &Pair) -> Option<Planned> {
    let provider = st.registry.provider(&pair.provider).ok()?;
    let declared = provider.models.iter().flatten().find(|m| {
        m.upstream_id.as_deref().filter(|u| !u.is_empty()).unwrap_or(&m.id) == pair.model
    });
    let requested = declared.map_or(pair.model.as_str(), |m| m.id.as_str());
    let ty = super::model_type(st, provider, requested).unwrap_or(ModelType::Text);
    Some(Planned { pair: pair.clone(), requested: requested.to_owned(), ty, skip: None })
}

/// Which retests are due. The clock is the caller's, so a test can drive it.
#[derive(Debug, Default)]
pub struct Retester {
    /// When each pair overdue at the first wake may run, `n × SPREAD` apart.
    spread: Option<HashMap<Pair, SystemTime>>,
    running: HashSet<Pair>,
    running_combos: HashSet<String>,
}

impl Retester {
    /// The retests due at `now` that can run, each marked running until [`Retester::done`],
    /// and when to look again.
    pub fn due(&mut self, engine: &Engine, now: SystemTime) -> (Vec<Planned>, SystemTime) {
        let st = engine.snapshot();
        let tests = &st.registry.runtime().tests;
        let board = engine.verdicts.snapshot();
        let mut dues: Vec<(SystemTime, Pair)> =
            board.all().into_iter().filter_map(|(p, v)| Some((due(v, tests)?, p))).collect();
        dues.sort();
        let spread = self.spread.get_or_insert_with(|| {
            let overdue = dues.iter().filter(|(at, _)| *at <= now);
            overdue.enumerate().map(|(n, (_, p))| (p.clone(), now + SPREAD * n as u32)).collect()
        });
        let mut out = Vec::new();
        let mut next = now + IDLE_WAKE;
        for (at, pair) in dues {
            if self.running.contains(&pair) {
                continue;
            }
            let at = spread.get(&pair).map_or(at, |s| at.max(*s));
            if at > now {
                next = next.min(at);
                continue;
            }
            // A waiting pair is looked at again on the next wake.
            if waiting(engine, &st, &pair, now).is_some() {
                continue;
            }
            let Some(p) = planned(&st, &pair) else { continue };
            spread.remove(&pair);
            self.running.insert(pair);
            out.push(p);
        }
        (out, next)
    }

    /// The combo retests due at `now` that can run, each marked running until
    /// [`Retester::combo_done`], and when to look again.
    pub fn due_combos(&mut self, engine: &Engine, now: SystemTime) -> (Vec<String>, SystemTime) {
        let st = engine.snapshot();
        let tests = &st.registry.runtime().tests;
        let board = engine.verdicts.snapshot();
        let mut out = Vec::new();
        let mut next = now + IDLE_WAKE;
        for (name, v) in &board.combos {
            let Some(at) = combo_due(v, tests) else { continue };
            if self.running_combos.contains(name) {
                continue;
            }
            if at > now {
                next = next.min(at);
                continue;
            }
            let Some(combo) = st.registry.combo(name) else { continue };
            if combo_waiting(engine, &st, combo).is_some() {
                continue;
            }
            self.running_combos.insert(name.clone());
            out.push(name.clone());
        }
        (out, next)
    }

    /// `pair`'s retest has finished.
    pub fn done(&mut self, pair: &Pair) {
        self.running.remove(pair);
    }

    /// `combo`'s retest has finished.
    pub fn combo_done(&mut self, combo: &str) {
        self.running_combos.remove(combo);
    }

    /// Retests in flight, combos included.
    pub fn running(&self) -> usize {
        self.running.len() + self.running_combos.len()
    }
}

/// A retest in flight.
enum Job {
    Pair(Pair),
    Combo(String),
}

/// Spawns the retest task over `engine`; it runs until `stop` completes, then cancels the
/// retests in flight and waits for them.
pub fn spawn(engine: Arc<Engine>, stop: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
    tokio::spawn(async move {
        let root = CancellationToken::new();
        let mut r = Retester::default();
        let mut jobs: JoinSet<Job> = JoinSet::new();
        tokio::pin!(stop);
        loop {
            while let Some(done) = jobs.try_join_next() {
                finished(&mut r, &jobs, done);
            }
            let (due, next) = r.due(&engine, SystemTime::now());
            for p in due {
                let (engine, token) = (engine.clone(), root.child_token());
                jobs.spawn(async move {
                    let run = super::run_id();
                    tokio::select! {
                        () = token.cancelled() => {}
                        _ = super::run_pair(&engine, &p, Source::Retest, &run, &token) => {}
                    }
                    Job::Pair(p.pair)
                });
            }
            let (combos, later) = r.due_combos(&engine, SystemTime::now());
            for name in combos {
                let (engine, token) = (engine.clone(), root.child_token());
                jobs.spawn(async move {
                    let run = super::run_id();
                    tokio::select! {
                        () = token.cancelled() => {}
                        _ = super::combo::run_combo(&engine, &name, Source::Retest, &run, &token) => {}
                    }
                    Job::Combo(name)
                });
            }
            let next = next.min(later);
            let wait = next.duration_since(SystemTime::now()).unwrap_or_default();
            tokio::select! {
                () = &mut stop => break,
                () = tokio::time::sleep(wait) => {}
                () = engine.changed.notified() => {}
                Some(done) = jobs.join_next(), if !jobs.is_empty() => finished(&mut r, &jobs, done),
            }
        }
        root.cancel();
        while jobs.join_next().await.is_some() {}
    })
}

fn finished(r: &mut Retester, jobs: &JoinSet<Job>, done: Result<Job, tokio::task::JoinError>) {
    match done {
        Ok(Job::Pair(pair)) => r.done(&pair),
        Ok(Job::Combo(name)) => r.combo_done(&name),
        Err(e) => {
            tracing::error!("retest failed: {e}");
            // The pair is lost with the task: once none run, none are marked.
            if jobs.is_empty() {
                r.running.clear();
                r.running_combos.clear();
            }
        }
    }
}
