//! Background maintenance (research R13): token refreshes, quota polls and live model lists,
//! run from one timer queue while the server is up, and the traffic tally checkpoints
//! (research R15): every 10 s and once more when the task stops.
//!
//! The queue holds one slot per [`JobKey`] (a [`JobKind`] and the account it serves). Each
//! wake, every kind says when each of its keys is next due ([`JobKind::due`]); the task
//! starts the due ones, at most [`MAX_JOBS`] at once, each with its own
//! [`CancellationToken`] (a child of the task's), and sleeps until the earliest next time,
//! a finished job, or an engine change (a reload, a token swap). Slots are rebuilt from the
//! current account set on every wake, so a reload drops removed accounts (cancelling their
//! running jobs) and keeps the last-run times of the others.
//!
//! Adding a job kind is a [`JobKind`] variant with an arm in [`JobKind::due`] and
//! [`JobKind::run`]; the queue itself doesn't change. Quota polls and live model lists keep
//! their own schedules (the engine's [`QuotaBoard`](crate::quota::poll::QuotaBoard) and
//! [`LiveModels`](crate::models_live::LiveModels)), so their due times include jitter and the
//! one retry after a failure.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nullrouter_registry::schema::SignInDecl;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::accounts::{Account, out_of_service};
use crate::models_live;
use crate::quota::poll;
use crate::signin::refresh::{self, Timing};
use crate::state::{Engine, EngineState};
use crate::tokens::TokenView;

/// Jobs running at once.
pub const MAX_JOBS: usize = 4;
/// The longest sleep without a due job; bounds a missed wake-up.
pub const IDLE_WAKE: Duration = Duration::from_secs(30);
/// The shortest gap between two runs of one job.
pub const MIN_GAP: Duration = Duration::from_millis(500);

/// What a job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JobKind {
    /// Refresh a sign-in account's tokens ahead of expiry (research R9).
    Refresh,
    /// Poll an account's provider-reported quota (research R11).
    QuotaPoll,
    /// Read a provider's live model list (research R14). The key's account is empty: the
    /// first active sign-in account is chosen when the job runs.
    ModelsLive,
}

impl JobKind {
    /// Every kind the queue asks.
    pub const ALL: [JobKind; 3] = [JobKind::Refresh, JobKind::QuotaPoll, JobKind::ModelsLive];

    /// Each key of this kind and when it's next due.
    pub fn due(self, engine: &Engine, st: &EngineState) -> Vec<(JobKey, SystemTime)> {
        match self {
            Self::Refresh => {
                let timing = engine.refresher.timing();
                st.accounts
                    .iter()
                    .filter(|a| !a.disabled)
                    .filter_map(|a| {
                        let decl = st.registry.provider(&a.provider).ok().and_then(|p| p.signin.as_ref());
                        let view = engine.tokens.get(&a.provider, &a.name);
                        let at = refresh_time(a, view.as_deref(), decl, &timing)?;
                        // After a transient failure, not before its backoff ends.
                        let at = engine.refresher.retry_at(&a.provider, &a.name).map_or(at, |r| at.max(r));
                        Some((JobKey::new(self, &a.provider, &a.name), at))
                    })
                    .collect()
            }
            Self::QuotaPoll => {
                let scale = engine.quota.timing().scale;
                st.accounts
                    .iter()
                    .filter(|a| !a.disabled && out_of_service(a, &st.tokens).is_none())
                    .filter(|a| st.registry.provider(&a.provider).is_ok_and(|p| poll::reported(p, a).is_some()))
                    .map(|a| {
                        let at = engine.quota.due(&a.provider, &a.name, a.poll_interval().mul_f64(scale));
                        (JobKey::new(self, &a.provider, &a.name), at)
                    })
                    .collect()
            }
            Self::ModelsLive => {
                let timing = engine.quota.timing();
                st.registry
                    .providers()
                    .filter_map(|p| {
                        let decl = p.models_live.as_ref()?;
                        models_live::first_signed_in(st, p)?;
                        let at = st.live_models.due(&p.id, decl.refresh.mul_f64(timing.scale), timing.retry_after);
                        Some((JobKey::new(self, &p.id, ""), at))
                    })
                    .collect()
            }
        }
    }

    /// Runs one job for `key`.
    pub async fn run(self, engine: Arc<Engine>, key: JobKey) {
        match self {
            Self::Refresh => {
                engine.refresh_account(&key.provider, &key.account).await;
            }
            Self::QuotaPoll => {
                engine.poll_quota(&key.provider, &key.account).await;
            }
            Self::ModelsLive => {
                let _ = engine.fetch_live_models(&key.provider).await;
            }
        }
    }
}

/// One job in the queue: its kind and the account (`provider/account`) it serves.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobKey {
    pub kind: JobKind,
    pub provider: String,
    pub account: String,
}

impl JobKey {
    pub fn new(kind: JobKind, provider: &str, account: &str) -> Self {
        Self { kind, provider: provider.to_owned(), account: account.to_owned() }
    }
}

/// When the queue refreshes `account`'s token: `expires_at − lead` ([`refresh::refresh_at`]).
/// `None` for a key account, an account with no tokens or no refresh token, a provider
/// without `[signin]`, and an account out of service for good.
pub fn refresh_time(
    account: &Account,
    view: Option<&TokenView>,
    decl: Option<&SignInDecl>,
    timing: &Timing,
) -> Option<SystemTime> {
    if !account.is_signin() {
        return None;
    }
    refresh::refresh_at(view?, decl?, timing)
}

#[derive(Debug, Default)]
struct Slot {
    last_run: Option<SystemTime>,
    running: Option<CancellationToken>,
}

/// The timer queue and its running jobs.
struct Queue {
    engine: Arc<Engine>,
    root: CancellationToken,
    slots: BTreeMap<JobKey, Slot>,
    jobs: JoinSet<JobKey>,
}

impl Queue {
    /// Starts the due jobs; returns when the next one is due.
    fn tick(&mut self) -> SystemTime {
        while let Some(done) = self.jobs.try_join_next() {
            self.finished(done);
        }
        let st = self.engine.snapshot();
        let wanted: BTreeMap<JobKey, SystemTime> = JobKind::ALL.iter().flat_map(|k| k.due(&self.engine, &st)).collect();
        // Rebuild: keys no longer wanted go (their jobs cancelled); the rest keep their
        // last-run times.
        self.slots.retain(|k, slot| {
            let keep = wanted.contains_key(k);
            if !keep && let Some(t) = &slot.running {
                t.cancel();
            }
            keep
        });
        let now = SystemTime::now();
        let mut next = now + IDLE_WAKE;
        for (key, due) in wanted {
            let slot = self.slots.entry(key.clone()).or_default();
            if slot.running.is_some() {
                continue;
            }
            let due = slot.last_run.map_or(due, |t| due.max(t + MIN_GAP));
            if due > now {
                next = next.min(due);
                continue;
            }
            if self.jobs.len() >= MAX_JOBS {
                // A finishing job wakes the queue.
                continue;
            }
            let token = self.root.child_token();
            slot.running = Some(token.clone());
            slot.last_run = Some(now);
            let engine = self.engine.clone();
            self.jobs.spawn(async move {
                tokio::select! {
                    () = token.cancelled() => {}
                    () = key.kind.run(engine, key.clone()) => {}
                }
                key
            });
        }
        next
    }

    fn finished(&mut self, done: Result<JobKey, tokio::task::JoinError>) {
        match done {
            Ok(key) => {
                if let Some(slot) = self.slots.get_mut(&key) {
                    slot.running = None;
                }
            }
            Err(e) => {
                tracing::error!("maintenance job failed: {e}");
                // The key is lost with the task: free every slot whose job has ended.
                let live = self.jobs.len();
                if live == 0 {
                    self.slots.values_mut().for_each(|s| s.running = None);
                }
            }
        }
    }

    async fn run(&mut self) {
        loop {
            let next = self.tick();
            let wait = next.duration_since(SystemTime::now()).unwrap_or_default();
            let engine = self.engine.clone();
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = engine.changed.notified() => {}
                Some(done) = self.jobs.join_next(), if !self.jobs.is_empty() => self.finished(done),
            }
        }
    }
}

/// Checkpoints the running tallies every [`History::checkpoint_every`] (research R15).
///
/// [`History::checkpoint_every`]: crate::quota::history::History::checkpoint_every
async fn checkpoints(engine: Arc<Engine>) {
    loop {
        tokio::time::sleep(engine.history.checkpoint_every()).await;
        engine.checkpoint_tallies().await;
    }
}

/// Spawns the maintenance task over `engine`; it runs until `stop` completes, then
/// cancels every running job, waits for them, and checkpoints the tallies a last time.
pub fn spawn(engine: Arc<Engine>, stop: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
    tokio::spawn(async move {
        let root = CancellationToken::new();
        let mut q = Queue { engine: engine.clone(), root: root.clone(), slots: BTreeMap::new(), jobs: JoinSet::new() };
        tokio::select! {
            () = stop => {}
            () = q.run() => {}
            () = checkpoints(engine.clone()) => {}
        }
        root.cancel();
        while q.jobs.join_next().await.is_some() {}
        engine.checkpoint_tallies().await;
    })
}
