//! Background maintenance (research R13): token refreshes, quota polls and live model lists,
//! run from one timer queue while the server is up.
//!
//! The queue holds one slot per [`JobKey`] (a [`JobKind`] and the account it serves). Each
//! wake, every kind says when each of its keys is next due ([`JobKind::due`]); the task
//! starts the due ones, at most [`MAX_JOBS`] at once, each with its own
//! [`CancellationToken`] (a child of the task's), and sleeps until the earliest next time,
//! a finished job, or an engine change (a reload, a token swap). Slots are rebuilt from the
//! current account set on every wake, so a reload drops removed accounts (cancelling their
//! running jobs) and keeps the last-run times of the others.
//!
//! Adding a job kind (US4: quota polls, live model lists) is a [`JobKind`] variant with an
//! arm in [`JobKind::due`] and [`JobKind::run`]; the queue itself doesn't change.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nullrouter_registry::schema::SignInDecl;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::accounts::Account;
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
}

impl JobKind {
    /// Every kind the queue asks.
    pub const ALL: [JobKind; 1] = [JobKind::Refresh];

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
        }
    }

    /// Runs one job for `key`.
    pub async fn run(self, engine: Arc<Engine>, key: JobKey) {
        match self {
            Self::Refresh => {
                engine.refresh_account(&key.provider, &key.account).await;
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

/// Spawns the maintenance task over `engine`; it runs until `stop` completes, then
/// cancels every running job and waits for them.
pub fn spawn(engine: Arc<Engine>, stop: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
    tokio::spawn(async move {
        let root = CancellationToken::new();
        let mut q = Queue { engine, root: root.clone(), slots: BTreeMap::new(), jobs: JoinSet::new() };
        tokio::select! {
            () = stop => {}
            () = q.run() => {}
        }
        root.cancel();
        while q.jobs.join_next().await.is_some() {}
    })
}
