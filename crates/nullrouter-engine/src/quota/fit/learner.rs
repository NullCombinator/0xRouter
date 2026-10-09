//! The learner: turns poll history into fitted numbers (research R1–R3, R5–R8, R11, R12).
//!
//! [`Fits::observe`] is the one entry point. It builds the new rows of each window from the
//! account's history, classifies them against the fit so far, refits the plugin window over the
//! rows of its epoch, runs separability and the split test, and moves each number from
//! `Learning` to `Fitted` when the always-valid test rejects the value in effect. It publishes
//! only the significant numbers into the [`Fits`] it is given.
//!
//! The learner's own state ([`Learner`]: rows, number states, splits, last fit) lives behind a
//! lock that only the poll path takes ([`Shared`]); requests read the published [`Fits`] and the
//! [`Meters`] built from it, and never see the learner.
//!
//! The value tested against is the value in effect: an operator override when one is set,
//! else the declared value (FR-020). A capacity is per account, so its override is the
//! account's. Weights and multipliers are pooled over the accounts, so they are tested against
//! the plugin-level override; an account's own weight override moves only that account's meter
//! and does not change what the pooled fit is tested against.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use arc_swap::ArcSwap;
use nullrouter_registry::Registry;
use nullrouter_registry::schema::{MeterDecl, MeterUnit, QuotaUnit, TokenWeights, glob_match};

use super::breaks::{self, Break};
use super::classify::{self, Inseparable, is_evidence, reclassify_epoch, separability};
use super::outside;
use super::linalg::Mat;
use super::model::{self, Fit, Kind, P, PriorTerm, Spec, Theta};
use super::rows::{Class, Row, SetAside, rows_from};
use super::split::{self, test_splits};
use super::store::{self, Loaded, Prior, Restart, SaveState, StoredFit, StoredNumber, StoredWindow};
use super::test::rejects;
use super::{Fits, MeterNumber, Meters, NumberState, Progress, TokenClass, WindowFit, WindowOverrides};
use crate::accounts::Accounts;
use crate::clock;
use crate::files::FileError;
use crate::quota::extract::rfc3339_millis;
use crate::quota::history::{self, Entry};
use crate::state::EngineState;

/// The provider's reading resolution: one percent point, or one unit.
const STEP: f64 = 1.0;
/// The most reclassify-and-refit rounds per poll, and split rounds per poll.
const SETTLE_ROUNDS: usize = 3;
const SPLIT_ROUNDS: usize = 2;
/// Reported for a number nothing informs yet.
const WIDE: f64 = 1.0e6;
/// Why a window restarts when its declared meter changed.
pub const METER_CHANGED: &str = "plugin_meter_changed";

/// Why the fit leaves an account alone: the provider declares no meters (priced per token).
pub const NOTE_PAYG: &str = "pay-as-you-go";
/// Why the fit leaves an account alone: the provider declares meters but reports no quota for it.
pub const NOTE_NO_QUOTA: &str = "provider reports no quota";

/// The note for an account the fit doesn't cover; `None` when it covers it.
pub fn unfitted_note(entity: &nullrouter_registry::ProviderEntity, account: &crate::accounts::Account) -> Option<&'static str> {
    if super::is_fitted_account(entity, account) {
        None
    } else if entity.routing().windows.is_empty() {
        Some(NOTE_PAYG)
    } else {
        Some(NOTE_NO_QUOTA)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn parse(s: &str) -> Option<SystemTime> {
    clock::parse_rfc3339(s)
}

/// One plugin window as the learner holds it.
#[derive(Debug, Clone)]
struct Win {
    epoch: SystemTime,
    hash: String,
    restarted: Option<(SystemTime, String)>,
    /// Every account's rows of the epoch, in arrival order.
    rows: Vec<Row>,
    /// Per account, the `at` of the newest good entry already turned into rows.
    cursors: BTreeMap<String, SystemTime>,
    /// Keyed like the store: `<number>` or `capacity@<account>`.
    numbers: BTreeMap<String, NumberState>,
    splits: BTreeMap<String, split::Split>,
    last: Option<(Spec, Fit)>,
    published: WindowFit,
    /// Each informed number's `(estimate, low, high)` in natural units, keyed like `numbers`.
    /// A weight on a percent window is the ratio to the input weight.
    ranges: BTreeMap<String, (f64, f64, f64)>,
    /// What was stored for the window; fields the learner doesn't own yet pass through.
    base: StoredWindow,
    /// Idle and busy rows already listed, keyed `(account, start)` to the entry id. Read from the
    /// account's outside-use file the first time the window lists, so a replay never lists a row
    /// twice (FR-021). Not stored: the file is the record.
    listed: BTreeMap<(String, String), String>,
    /// Accounts whose file `listed` has been read from.
    listed_read: BTreeSet<String>,
}

impl Win {
    fn fresh(epoch: SystemTime, hash: &str, restarted: Option<(SystemTime, String)>) -> Self {
        Self {
            epoch,
            hash: hash.to_owned(),
            restarted,
            rows: Vec::new(),
            cursors: BTreeMap::new(),
            numbers: BTreeMap::new(),
            splits: BTreeMap::new(),
            last: None,
            published: WindowFit::default(),
            ranges: BTreeMap::new(),
            base: StoredWindow::default(),
            listed: BTreeMap::new(),
            listed_read: BTreeSet::new(),
        }
    }

    fn restored(s: &StoredWindow, now: SystemTime) -> Self {
        let mut w = Self::fresh(parse(&s.epoch).unwrap_or(now), &s.meter_hash, None);
        w.restarted = s.restarted.as_ref().and_then(|r| Some((parse(&r.at)?, r.reason.clone())));
        for (key, n) in &s.numbers {
            let Some(since) = n.since.as_deref().and_then(parse) else { continue };
            match n.state.as_str() {
                "fitted" => {
                    w.numbers.insert(key.clone(), NumberState::Fitted { since });
                }
                "relearning" => {
                    w.numbers.insert(
                        key.clone(),
                        NumberState::Relearning { since, progress: Progress { intervals: 0, half_width: WIDE } },
                    );
                }
                _ => {}
            }
        }
        for (account, sp) in &s.splits {
            w.splits.insert(
                account.clone(),
                split::Split { since: parse(&sp.since).unwrap_or(now), reason: sp.reason.clone() },
            );
        }
        w.base = s.clone();
        w
    }

    fn to_stored(&self) -> StoredWindow {
        let mut w = self.base.clone();
        w.epoch = rfc3339_millis(self.epoch);
        w.meter_hash = self.hash.clone();
        w.restarted = self.restarted.as_ref().map(|(t, r)| Restart { at: rfc3339_millis(*t), reason: r.clone() });
        w.numbers = self.numbers.iter().filter_map(|(k, s)| Some((k.clone(), stored_number(s)?))).collect();
        w.splits = self
            .splits
            .iter()
            .map(|(a, s)| (a.clone(), store::Split { since: rfc3339_millis(s.since), reason: s.reason.clone() }))
            .collect();
        w
    }
}

fn stored_number(s: &NumberState) -> Option<StoredNumber> {
    let (state, since) = match s {
        NumberState::Fitted { since } => ("fitted", Some(rfc3339_millis(*since))),
        NumberState::NotSeparable { .. } => ("not_separable", None),
        NumberState::Relearning { since, .. } => ("relearning", Some(rfc3339_millis(*since))),
        NumberState::Learning { .. } | NumberState::Restarted { .. } => ("learning", None),
        NumberState::Yardstick | NumberState::NotReported | NumberState::NotFitted(_) => return None,
    };
    Some(StoredNumber { state: state.to_owned(), since })
}

/// Rows set aside in a window, per reason (contracts/operator-socket.md `set_aside`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SetAsideCounts {
    pub reset: u64,
    pub usage_unreported: u64,
    pub exhausted: u64,
}

/// What a view reads of one plugin window, copied out of the learner.
#[derive(Debug, Clone)]
pub struct WindowSnapshot {
    pub epoch: SystemTime,
    /// A weighted-token window reported as percent: weights are ratios to the input weight.
    pub percent: bool,
    /// Keyed like the store: `<number>` or `capacity@<account>`.
    pub numbers: BTreeMap<String, NumberState>,
    /// `(estimate, low, high)` per informed number, keyed like `numbers`.
    pub ranges: BTreeMap<String, (f64, f64, f64)>,
    /// Accounts split off, with when and why.
    pub splits: BTreeMap<String, (SystemTime, String)>,
    /// Per account.
    pub set_aside: BTreeMap<String, SetAsideCounts>,
}

/// The learner's mutable state, for every plugin. Only the poll path touches it.
#[derive(Debug, Default)]
pub struct Learner {
    windows: BTreeMap<(String, String), Win>,
    /// Per provider, what the store held when last loaded or saved.
    stored: BTreeMap<String, StoredFit>,
    /// Providers whose store couldn't be read or set aside: never saved over.
    blocked: BTreeSet<String>,
    /// Whether the last save of each provider reached the disk.
    pub saves: BTreeMap<String, SaveState>,
    /// Accounts the fit leaves alone, with the reason (FR-029).
    unfitted: BTreeMap<(String, String), String>,
}

impl Fits {
    /// Learns from `account`'s history. `history_tail` holds the account's entries, oldest
    /// first, from at least [`Learner::since`] on; entries already seen are skipped. Updates
    /// `self` (the significant numbers of `provider`) and returns whether it changed, in which
    /// case the caller rebuilds the meters. Saves the fit state when it changed.
    #[allow(clippy::too_many_arguments)]
    pub fn observe(
        &mut self,
        learner: &mut Learner,
        home: &Path,
        st: &EngineState,
        provider: &str,
        account: &str,
        history_tail: &[Entry],
        now: SystemTime,
    ) -> bool {
        learner.run(self, home, st, provider, &[(account, history_tail)], now)
    }
}

impl Learner {
    /// The `at` the account's history must be read from for [`Fits::observe`]: its cursor
    /// (or the epoch) in the oldest window. `None`: read everything.
    pub fn since(&mut self, home: &Path, provider: &str, account: &str) -> Option<SystemTime> {
        self.load_store(home, provider);
        let mine: Vec<&Win> = self.windows.iter().filter(|((p, _), _)| p == provider).map(|(_, w)| w).collect();
        if mine.is_empty() {
            let stored = self.stored.get(provider)?;
            if stored.windows.is_empty() {
                return None;
            }
            let epochs: Option<Vec<SystemTime>> = stored.windows.values().map(|w| parse(&w.epoch)).collect();
            return epochs?.into_iter().min();
        }
        mine.iter().map(|w| w.cursors.get(account).copied().unwrap_or(w.epoch)).min()
    }

    /// The state of every number of one window, keyed like the store.
    pub fn number_states(&self, provider: &str, window: &str) -> BTreeMap<String, NumberState> {
        self.windows.get(&(provider.to_owned(), window.to_owned())).map(|w| w.numbers.clone()).unwrap_or_default()
    }

    /// The 95% range `(estimate, low, high)` of every informed number of one window, in natural
    /// units and keyed like [`Learner::number_states`]. A weight on a percent window is the ratio
    /// to the input weight.
    pub fn ranges(&self, provider: &str, window: &str) -> BTreeMap<String, (f64, f64, f64)> {
        self.windows.get(&(provider.to_owned(), window.to_owned())).map(|w| w.ranges.clone()).unwrap_or_default()
    }

    /// The accounts split off from `window`'s pooled numbers, with their reasons.
    pub fn splits(&self, provider: &str, window: &str) -> BTreeMap<String, String> {
        self.windows
            .get(&(provider.to_owned(), window.to_owned()))
            .map(|w| w.splits.iter().map(|(a, s)| (a.clone(), s.reason.clone())).collect())
            .unwrap_or_default()
    }

    /// Every window the learner holds, by `(provider, window)`, as owned copies.
    pub fn snapshot_all(&self) -> BTreeMap<(String, String), WindowSnapshot> {
        self.windows
            .iter()
            .map(|(key, w)| {
                let mut set_aside: BTreeMap<String, SetAsideCounts> = BTreeMap::new();
                for r in &w.rows {
                    if let Class::SetAside(why) = r.class {
                        let c = set_aside.entry(r.account.clone()).or_default();
                        match why {
                            SetAside::Reset => c.reset += 1,
                            SetAside::UsageUnreported => c.usage_unreported += 1,
                            SetAside::Exhausted => c.exhausted += 1,
                        }
                    }
                }
                let snap = WindowSnapshot {
                    epoch: w.epoch,
                    percent: w.last.as_ref().is_some_and(|(s, _)| s.kind == Kind::Percent),
                    numbers: w.numbers.clone(),
                    ranges: w.ranges.clone(),
                    splits: w.splits.iter().map(|(a, s)| (a.clone(), (s.since, s.reason.clone()))).collect(),
                    set_aside,
                };
                (key.clone(), snap)
            })
            .collect()
    }

    /// Why the fit doesn't cover `account`: no quota reports, or pay-as-you-go. `None`: it does.
    pub fn not_fitted(&self, provider: &str, account: &str) -> Option<NumberState> {
        self.unfitted.get(&(provider.to_owned(), account.to_owned())).map(|r| NumberState::NotFitted(r.clone()))
    }

    /// The note a view shows for `window` when every account is split off, so no pooled fit
    /// remains; `None` otherwise.
    pub fn pooled_note(&self, provider: &str, window: &str) -> Option<&'static str> {
        let win = self.windows.get(&(provider.to_owned(), window.to_owned()))?;
        let (spec, _) = win.last.as_ref()?;
        let all = !spec.accounts.is_empty() && spec.accounts.iter().all(|a| win.splits.contains_key(a));
        all.then_some(super::NO_POOLED_FIT)
    }

    fn load_store(&mut self, home: &Path, provider: &str) {
        if self.stored.contains_key(provider) {
            return;
        }
        let stored = match store::load(home, provider) {
            Ok(Loaded::Missing) => StoredFit::default(),
            Ok(Loaded::Ok(f)) => f,
            Ok(Loaded::Bad { renamed_to }) => {
                tracing::warn!(provider, "fit state didn't parse and was set aside as {}; fits restart", renamed_to.display());
                StoredFit::default()
            }
            Err(e) => {
                tracing::warn!(provider, "fit state not read, and kept as it is: {e}");
                self.blocked.insert(provider.to_owned());
                StoredFit::default()
            }
        };
        self.stored.insert(provider.to_owned(), stored);
    }

    /// Runs the learner over `tails` (several accounts at replay, one when live) for every
    /// window of `provider`. Returns whether `fits` changed.
    fn run(
        &mut self,
        fits: &mut Fits,
        home: &Path,
        st: &EngineState,
        provider: &str,
        tails: &[(&str, &[Entry])],
        now: SystemTime,
    ) -> bool {
        let Some(entity) = st.registry.providers().find(|p| p.id == provider) else { return false };
        // Only polled accounts are fitted; the others are listed as not fitted.
        let (fitted, unfitted): (Vec<_>, Vec<_>) = st.accounts.for_provider(provider).partition(|a| super::is_fitted_account(entity, a));
        self.unfitted.retain(|(p, _), _| p != provider);
        for a in unfitted {
            let note = unfitted_note(entity, a).unwrap_or(NOTE_NO_QUOTA);
            self.unfitted.insert((provider.to_owned(), a.name.clone()), note.to_owned());
        }
        let accounts: Vec<String> = fitted.iter().map(|a| a.name.clone()).collect();
        let ov = super::window_overrides(st, provider);
        self.run_declared(fits, home, provider, entity.routing().windows, &accounts, &ov, tails, now)
    }

    /// [`Fits::observe`] without an engine snapshot: `declared` are the plugin's windows and
    /// `accounts` its polled accounts. The simulated week drives the learner through this.
    #[allow(clippy::too_many_arguments)]
    pub fn observe_declared(
        &mut self,
        fits: &mut Fits,
        home: &Path,
        provider: &str,
        declared: &[MeterDecl],
        accounts: &[String],
        tails: &[(&str, &[Entry])],
        now: SystemTime,
    ) -> bool {
        self.run_declared(fits, home, provider, declared, accounts, &BTreeMap::new(), tails, now)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_declared(
        &mut self,
        fits: &mut Fits,
        home: &Path,
        provider: &str,
        declared: &[MeterDecl],
        accounts: &[String],
        overrides: &BTreeMap<String, WindowOverrides>,
        tails: &[(&str, &[Entry])],
        now: SystemTime,
    ) -> bool {
        let mut accounts = accounts.to_vec();
        accounts.sort();
        if !tails.iter().any(|(a, _)| accounts.iter().any(|x| x.as_str() == *a)) {
            return false;
        }
        self.load_store(home, provider);
        for meter in declared {
            self.window(home, provider, declared, meter, &accounts, overrides.get(&meter.name), tails, now);
        }
        // A window the plugin no longer declares is forgotten.
        self.windows.retain(|(p, w), _| p != provider || declared.iter().any(|m| m.name == *w));
        let mut out: BTreeMap<String, WindowFit> = BTreeMap::new();
        for ((p, w), win) in &self.windows {
            if p == provider
                && !(win.published.capacity.is_empty()
                    && win.published.weights.is_empty()
                    && win.published.multipliers.is_empty())
            {
                out.insert(w.clone(), win.published.clone());
            }
        }
        let changed = if out.is_empty() {
            fits.windows.remove(provider).is_some()
        } else if fits.windows.get(provider) != Some(&out) {
            fits.windows.insert(provider.to_owned(), out);
            true
        } else {
            false
        };
        self.persist(home, provider, declared, now);
        changed
    }

    /// One window: new rows, classification, refit, splits, number states, published numbers.
    fn window(&mut self, home: &Path, provider: &str, declared: &[MeterDecl], meter: &MeterDecl, accounts: &[String], ov: Option<&WindowOverrides>, tails: &[(&str, &[Entry])], now: SystemTime) {
        let none = WindowOverrides::default();
        let ov = ov.unwrap_or(&none);
        let matches = |name: &str| name == meter.name || glob_match(&meter.name, name);
        // The unit the provider reports the window in, from the newest good entry.
        let unit = tails
            .iter()
            .flat_map(|(_, tail)| tail.iter().rev().filter(|e| e.ok))
            .find_map(|e| e.windows.iter().find(|w| matches(&w.name)).map(|w| w.unit));
        let Some(unit) = unit else { return };
        let hash = store::meter_hash(meter);
        // A capacity the plugin doesn't declare is assumed from peers; the fit starts from it.
        let assumed = assumed_meter(declared, meter);
        let meter = &assumed;
        let key = (provider.to_owned(), meter.name.clone());
        let changed = self.windows.get(&key).is_some_and(|w| w.hash != hash);
        if changed || !self.windows.contains_key(&key) {
            let win = self.make_win(provider, meter, &hash, tails, now, changed);
            self.windows.insert(key.clone(), win);
        }
        let Some(win) = self.windows.get_mut(&key) else { return };

        let percent = unit == QuotaUnit::Percent;
        let kind = match (meter.unit, percent) {
            (MeterUnit::WeightedTokens, true) => Kind::Percent,
            (MeterUnit::WeightedTokens, false) => Kind::Counted,
            (MeterUnit::Requests, true) => Kind::RequestsPercent,
            (MeterUnit::Requests, false) => Kind::RequestsCounted,
        };
        let spec = Spec {
            kind,
            accounts: accounts.to_vec(),
            globs: meter.model_multiplier.keys().cloned().collect(),
            utc_offset_secs: 0,
        };
        // A fit over other accounts or globs can't start this one.
        if win.last.as_ref().is_some_and(|(s, _)| *s != spec) {
            win.last = None;
        }

        // New rows, each classified against the fit before it joins it (R6).
        let mut added = 0;
        for (account, tail) in tails {
            if !accounts.iter().any(|a| a.as_str() == *account) {
                continue;
            }
            let floor = win.base.account_epochs.get(*account).and_then(|e| parse(e)).map_or(win.epoch, |e| e.max(win.epoch));
            let since = win.cursors.get(*account).copied().unwrap_or(floor);
            let fit = win.last.as_ref().map(|(_, f)| f);
            let mut new = rows_from(account, tail, meter, since);
            for row in &mut new {
                row.class = classify::classify(row, &spec, fit, STEP);
            }
            added += new.len();
            win.rows.extend(new);
            let newest = tail.iter().rev().filter(|e| e.ok && e.windows.iter().any(|w| matches(&w.name))).find_map(Entry::time);
            if let Some(t) = newest {
                let cursor = win.cursors.entry((*account).to_owned()).or_insert(t);
                *cursor = (*cursor).max(t);
            }
        }
        if added == 0 {
            return;
        }
        refit(win, &spec, meter, ov, provider, now);
        list_outside_rows(win, home, provider, &spec, meter, unit, now);
        list_steady_rates(win, home, provider, &spec, meter, unit, now);
    }

    /// A window seen for the first time in this run: restored from the store, restarted
    /// because the declared meter changed (FR-017), or new.
    fn make_win(
        &self,
        provider: &str,
        meter: &MeterDecl,
        hash: &str,
        tails: &[(&str, &[Entry])],
        now: SystemTime,
        meter_changed: bool,
    ) -> Win {
        let restart = || Win::fresh(now, hash, Some((now, METER_CHANGED.to_owned())));
        if meter_changed {
            return restart();
        }
        match self.stored.get(provider).and_then(|f| f.windows.get(&meter.name)) {
            Some(s) if s.meter_hash == hash => Win::restored(s, now),
            Some(_) => restart(),
            None => {
                let first = tails.iter().filter_map(|(_, t)| t.iter().find_map(Entry::time)).min();
                Win::fresh(first.unwrap_or(now), hash, None)
            }
        }
    }

    /// Saves the provider's fit state when it differs from what is stored.
    fn persist(&mut self, home: &Path, provider: &str, declared: &[MeterDecl], now: SystemTime) {
        if self.blocked.contains(provider) {
            return;
        }
        let Some(stored) = self.stored.get_mut(provider) else { return };
        let mut windows = stored.windows.clone();
        windows.retain(|name, _| declared.iter().any(|m| m.name == *name));
        for ((p, w), win) in &self.windows {
            if p == provider {
                windows.insert(w.clone(), win.to_stored());
            }
        }
        if stored.windows == windows {
            return;
        }
        let next = StoredFit { v: store::VERSION, windows };
        let result = store::save(home, provider, &next);
        match &result {
            Ok(()) => *stored = next,
            Err(e) => tracing::warn!(provider, "fit state not saved: {e}"),
        }
        self.saves.entry(provider.to_owned()).or_default().record(result, now);
    }
}

fn evidence(win: &Win) -> Vec<Row> {
    win.rows.iter().filter(|r| is_evidence(r.class)).cloned().collect()
}

fn split_indices(win: &Win, spec: &Spec) -> BTreeSet<usize> {
    win.splits.keys().filter_map(|a| spec.accounts.iter().position(|x| x == a)).collect()
}

/// The weight `class` carries in the declaration.
fn weight_of(w: &TokenWeights, class: TokenClass) -> f64 {
    match class {
        TokenClass::Input => w.input,
        TokenClass::Output => w.output,
        TokenClass::CacheRead => w.cache_read,
        TokenClass::CacheWrite => w.cache_write,
    }
}

/// The meter with a missing capacity filled from its peers: the median capacity of the other
/// windows of the same length and unit (the routing core's rule, research R5). Unchanged when
/// the capacity is declared or no peer has one.
fn assumed_meter(declared: &[MeterDecl], meter: &MeterDecl) -> MeterDecl {
    let mut out = meter.clone();
    if meter.capacity.is_some() {
        return out;
    }
    let mut peers: Vec<f64> = declared
        .iter()
        .filter(|m| m.name != meter.name && m.length == meter.length && m.unit == meter.unit)
        .filter_map(|m| m.capacity)
        .filter(|c| c.is_finite() && *c > 0.0)
        .collect();
    peers.sort_by(f64::total_cmp);
    out.capacity = peers.get(peers.len() / 2).copied();
    out
}

/// Where the Gauss-Newton run starts: the declared meter.
fn start_theta(spec: &Spec, meter: &MeterDecl) -> Theta {
    let mut th = Theta::neutral(spec.accounts.len(), spec.globs.len());
    let positive = |v: f64| if v.is_finite() && v > 0.0 { v } else { 1e-6 };
    let w = meter.token_weights.unwrap_or_default();
    let cap = meter.capacity.filter(|c| c.is_finite() && *c > 0.0);
    let input = positive(w.input);
    match spec.kind {
        Kind::Percent => {
            th.k = vec![cap.map_or(1e-4, |c| 100.0 * input / c); spec.accounts.len()];
            th.rho = [positive(w.output) / input, positive(w.cache_read) / input, positive(w.cache_write) / input];
        }
        Kind::RequestsPercent => th.k = vec![cap.map_or(1e-2, |c| 100.0 / c); spec.accounts.len()],
        Kind::Counted => th.w = [input, positive(w.output), positive(w.cache_read), positive(w.cache_write)],
        Kind::RequestsCounted => {}
    }
    th.mu = meter.model_multiplier.values().map(|f| positive(*f)).collect();
    th
}

/// After a refit: lists each idle row classified `Outside` as an `idle` entry (amount: the whole
/// change) and each busy row now final `Outside` as a `busy` entry (amount: the excess beyond the
/// upper range, step included). A provisional busy row is not listed. Each row is listed once: the
/// account's file is read the first time, and rows whose `(account, start)` it holds are skipped.
/// A listed row that counts as evidence again gets a `reclassified` line and may be listed anew
/// if it turns outside again. An account whose file can't be read or written is left for the
/// next refit. Raises no alerts and logs no outside use (FR-022).
fn list_outside_rows(win: &mut Win, home: &Path, provider: &str, spec: &Spec, meter: &MeterDecl, unit: QuotaUnit, now: SystemTime) {
    let Some((_, fit)) = &win.last else { return };
    for account in &spec.accounts {
        if !win.listed_read.contains(account) {
            match outside::read(home, provider, account, None, None) {
                Ok(list) => {
                    for e in list {
                        if e.window == meter.name && matches!(e.ty, outside::OutsideType::Idle | outside::OutsideType::Busy) {
                            win.listed.insert((account.clone(), e.start), e.id);
                        }
                    }
                    win.listed_read.insert(account.clone());
                }
                Err(e) => {
                    tracing::warn!(provider, account = account.as_str(), "outside use not read: {e}");
                    continue;
                }
            }
        }
        let mut lines = Vec::new();
        let mut added: Vec<((String, String), String)> = Vec::new();
        let mut withdrawn: Vec<(String, String)> = Vec::new();
        for row in win.rows.iter().filter(|r| r.account == *account) {
            let key = (account.clone(), rfc3339_millis(row.start));
            if row.class == Class::Outside {
                if win.listed.contains_key(&key) {
                    continue;
                }
                let (ty, amount) = if row.has_traffic() {
                    let Some(m) = model::prepare(spec, std::slice::from_ref(row)).into_iter().next() else { continue };
                    (outside::OutsideType::Busy, (row.y - classify::upper(fit, spec, &m, STEP)).max(0.0))
                } else {
                    (outside::OutsideType::Idle, row.y)
                };
                let (id, line) = outside::interval_line(&meter.name, unit.as_str(), ty, (row.start, row.end), amount, now);
                lines.push(line);
                added.push((key, id));
            } else if let Some(id) = win.listed.get(&key) {
                lines.push(outside::reclassified_line(id, "counts as evidence again", now));
                withdrawn.push(key);
            }
        }
        match outside::append(home, provider, account, &lines) {
            Ok(()) => {
                for key in withdrawn {
                    win.listed.remove(&key);
                }
                win.listed.extend(added);
            }
            Err(e) => tracing::warn!(provider, account = account.as_str(), "outside use not listed: {e}"),
        }
    }
}

/// After a refit: appends a `steady` outside-use entry for each account's part-of-day rate that
/// is clearly above 0 and new or changed since the last one listed (FR-021). Written for every
/// polled account; alerts and log lines are the detector's business and need an exclusive-use
/// declaration (FR-022). A rate is recorded in `alerted_rates` only once its line is written.
fn list_steady_rates(win: &mut Win, home: &Path, provider: &str, spec: &Spec, meter: &MeterDecl, unit: QuotaUnit, now: SystemTime) {
    let Some((_, fit)) = &win.last else { return };
    for (a, account) in spec.accounts.iter().enumerate() {
        for q in 0..model::PARTS {
            let p = P::B(a, q);
            if !fit.active.contains(&p) {
                continue;
            }
            let Some(se) = fit.se(p) else { continue };
            let est = fit.theta.get(p);
            let Some((key, rate, line)) =
                outside::steady_line(&win.base.alerted_rates, &meter.name, unit.as_str(), account, q, (est, se), win.epoch, now)
            else {
                continue;
            };
            match outside::append(home, provider, account, &[line]) {
                Ok(()) => {
                    win.base.alerted_rates.insert(key, rate);
                }
                Err(e) => tracing::warn!(provider, account = account.as_str(), "steady rate not listed: {e}"),
            }
        }
    }
}

/// Refits the pool over the epoch's evidence, reclassifies, tests splits, then updates the
/// numbers (R3, R6, R8).
fn refit(win: &mut Win, spec: &Spec, meter: &MeterDecl, ov: &WindowOverrides, provider: &str, now: SystemTime) {
    let mut splits_found = 0;
    loop {
        if !settle(win, spec, meter, now) {
            return;
        }
        let Some((_, fit)) = &win.last else { return };
        let all = model::prepare(spec, &evidence(win));
        let new = test_splits(spec, &all, fit, &split_indices(win, spec), now);
        if new.is_empty() || splits_found >= SPLIT_ROUNDS {
            break;
        }
        for (a, s) in new {
            if let Some(name) = spec.accounts.get(a) {
                win.splits.insert(name.clone(), s);
            }
        }
        splits_found += 1;
    }
    update_numbers(win, spec, meter, ov, now);
    // A break moves the epoch: refit what remains of it (R9).
    if detect_breaks(win, spec, meter, ov, provider, now) && settle(win, spec, meter, now) {
        update_numbers(win, spec, meter, ov, now);
    }
}

/// Runs the break detector over the window's fitted numbers and applies what it finds. Returns
/// whether a break was recorded.
fn detect_breaks(win: &mut Win, spec: &Spec, meter: &MeterDecl, ov: &WindowOverrides, provider: &str, now: SystemTime) -> bool {
    let Some((_, fit)) = &win.last else { return false };
    let targets: Vec<breaks::Target> = numbers(spec, meter, ov)
        .into_iter()
        .filter(|n| matches!(win.numbers.get(&n.key), Some(NumberState::Fitted { .. })) && fit.active.contains(&n.p))
        .map(|n| breaks::Target { number: n.number, p: n.p, account: n.account, scale: n.scale })
        .collect();
    if targets.is_empty() {
        return false;
    }
    let account_epochs: BTreeMap<String, SystemTime> =
        win.base.account_epochs.iter().filter_map(|(a, e)| Some((a.clone(), parse(e)?))).collect();
    let found = breaks::detect(&meter.name, spec, &win.rows, &split_indices(win, spec), fit, &targets, win.epoch, &account_epochs, now);
    if found.is_empty() {
        return false;
    }
    apply_breaks(win, &found, provider);
    true
}

/// Most breaks a window keeps in its stored record.
const KEEP_BREAKS: usize = 64;

/// Records `found`: a pooled number's break restarts the whole window at its time (every fitted
/// number goes to `Relearning`, rows and splits before it are dropped); a capacity's restarts
/// only that account. Rows of the span that were set aside as busy outside use count as evidence
/// again; the ones already listed get their `reclassified` line at the next listing pass.
fn apply_breaks(win: &mut Win, found: &[Break], provider: &str) {
    let relearn = |since| NumberState::Relearning { since, progress: Progress { intervals: 0, half_width: WIDE } };
    for b in found {
        breaks::log(provider, b);
        win.base.breaks.push(store::StoredBreak {
            at: rfc3339_millis(b.at),
            detected_at: rfc3339_millis(b.detected_at),
            number: b.key(),
            replaced: b.replaced,
        });
    }
    let excess = win.base.breaks.len().saturating_sub(KEEP_BREAKS);
    win.base.breaks.drain(..excess);

    let pooled_at = found.iter().filter(|b| b.account.is_none()).map(|b| b.at).max();
    if let Some(at) = pooled_at {
        win.epoch = at;
        win.rows.retain(|r| r.start >= at);
        win.splits.clear();
        // What was pruned from the old epoch counts for nothing in the new one.
        win.base.prior = None;
        for state in win.numbers.values_mut() {
            if matches!(state, NumberState::Fitted { .. }) {
                *state = relearn(at);
            }
        }
        win.published = WindowFit { relative_to_input: win.published.relative_to_input, ..WindowFit::default() };
        win.ranges.clear();
    }
    let mut from: BTreeMap<String, SystemTime> = BTreeMap::new();
    for b in found {
        let Some(account) = &b.account else { continue };
        if pooled_at.is_some_and(|p| p >= b.at) {
            continue;
        }
        win.rows.retain(|r| r.account != *account || r.start >= b.at);
        win.base.account_epochs.insert(account.clone(), rfc3339_millis(b.at));
        win.numbers.insert(b.key(), relearn(b.at));
        win.published.capacity.remove(account);
        win.ranges.remove(&b.key());
        from.insert(account.clone(), b.at);
    }
    for r in &mut win.rows {
        let cut = [pooled_at, from.get(&r.account).copied()].into_iter().flatten().min();
        if cut.is_some_and(|c| r.start >= c) && r.has_traffic() && matches!(r.class, Class::Outside | Class::OutsideProvisional { .. }) {
            r.class = Class::Evidence;
        }
    }
    // The fit blended the old rules in; the next one starts from the new epoch's rows.
    win.last = None;
}

/// Fits the pool and reclassifies the epoch against the fit until the classes stop moving.
/// `false` when no fit could be made.
fn settle(win: &mut Win, spec: &Spec, meter: &MeterDecl, now: SystemTime) -> bool {
    let mut fitted = false;
    for _ in 0..SETTLE_ROUNDS {
        let rows = model::prepare(spec, &evidence(win));
        let pool = split::without(&rows, &split_indices(win, spec));
        let start = match &win.last {
            Some((_, f)) => f.theta.clone(),
            None => start_theta(spec, meter),
        };
        let prior = win.base.prior.as_ref().map(prior_term);
        let Some(fit) = model::fit_with_prior(spec, &pool, &start, prior.as_ref()) else { return fitted };
        let before: Vec<Class> = win.rows.iter().map(|r| r.class).collect();
        reclassify_epoch(&mut win.rows, spec, Some(&fit), STEP, now);
        win.last = Some((spec.clone(), fit));
        fitted = true;
        if win.rows.iter().map(|r| r.class).eq(before) {
            break;
        }
    }
    fitted
}

fn prior_term(p: &Prior) -> PriorTerm {
    PriorTerm { params: p.params.clone(), mean: p.mean.clone(), information: p.information.clone() }
}

/// The estimate and information of a fit over rows about to be pruned, as stored.
fn prior_of(spec: &Spec, fit: &Fit, through: SystemTime) -> Prior {
    let n = fit.active.len();
    Prior {
        through: rfc3339_millis(through),
        params: fit.active.iter().map(|p| model::param_name(spec, *p)).collect(),
        mean: fit.active.iter().map(|p| model::fit_space(*p, fit.theta.get(*p))).collect(),
        information: (0..n).map(|i| (0..n).map(|j| fit.info[(i, j)]).collect()).collect(),
    }
}

fn prior_shaped(p: &Prior) -> bool {
    let n = p.params.len();
    p.mean.len() == n && p.information.len() == n && p.information.iter().all(|r| r.len() == n)
}

/// Two priors as one: informations add, the mean is their information-weighted average over the
/// union of the parameters. An `old` that is malformed, or a sum that is singular, leaves `new`.
fn combine(old: &Prior, new: &Prior) -> Prior {
    if !prior_shaped(old) || !prior_shaped(new) {
        return new.clone();
    }
    let mut names = old.params.clone();
    for n in &new.params {
        if !names.contains(n) {
            names.push(n.clone());
        }
    }
    let m = names.len();
    let mut info = Mat::zeros(m, m);
    let mut rhs = vec![0.0; m];
    for pr in [old, new] {
        let idx: Vec<usize> = pr.params.iter().filter_map(|n| names.iter().position(|x| x == n)).collect();
        for (j, &a) in idx.iter().enumerate() {
            for (k, &b) in idx.iter().enumerate() {
                info[(a, b)] += pr.information[j][k];
                rhs[a] += pr.information[j][k] * pr.mean[k];
            }
        }
    }
    let mut ridged = info.clone();
    let ridge = 1e-12 * (0..m).map(|i| info[(i, i)]).sum::<f64>() / m.max(1) as f64 + 1e-300;
    for i in 0..m {
        ridged[(i, i)] += ridge;
    }
    let Some(mean) = ridged.solve(&rhs).filter(|v| v.iter().all(|x| x.is_finite())) else { return new.clone() };
    Prior {
        through: new.through.clone(),
        params: names,
        mean,
        information: (0..m).map(|i| (0..m).map(|j| info[(i, j)]).collect()).collect(),
    }
}

/// Folds the rows `quota prune --before <before>` is about to delete into each window's prior
/// (research R11): for every window of the matching providers, the evidence rows of the matching
/// accounts that start before `before` (and after the window's epoch and any earlier fold) are
/// fitted alone, and their estimate and information are combined with the stored prior and saved
/// to `quota/fit/<provider>.json`. Run it before the prune; a repeat folds nothing twice, since
/// a fold covers the rows starting before its `through`. Returns how many windows were updated.
/// A window with too few rows to fit alone is not folded and is logged. A running server holds
/// its own copy of the fit file and may save over this one: stop it, or prune and restart.
pub fn fold_prior(
    home: &Path,
    registry: &Registry,
    accounts: &Accounts,
    provider: Option<&str>,
    account: Option<&str>,
    before: SystemTime,
) -> Result<usize, FileError> {
    let now = clock::now();
    let mut folded = 0;
    for entity in registry.providers() {
        if provider.is_some_and(|p| p != entity.id) {
            continue;
        }
        let declared = entity.routing().windows;
        if declared.is_empty() {
            continue;
        }
        let mut names: Vec<String> =
            accounts.for_provider(&entity.id).filter(|a| super::is_fitted_account(entity, a)).map(|a| a.name.clone()).collect();
        names.sort();
        let mut tails: Vec<(String, Vec<Entry>)> = Vec::new();
        for n in &names {
            let t = history::read(home, &entity.id, n, None, None)?;
            if !t.is_empty() {
                tails.push((n.clone(), t));
            }
        }
        if !tails.iter().any(|(n, _)| account.is_none_or(|a| a == n.as_str())) {
            continue;
        }
        let mut stored = match store::load(home, &entity.id)? {
            Loaded::Ok(f) => f,
            Loaded::Missing | Loaded::Bad { .. } => StoredFit::default(),
        };
        let mut changed = false;
        for meter in declared {
            let matches = |name: &str| name == meter.name || glob_match(&meter.name, name);
            let unit = tails
                .iter()
                .flat_map(|(_, t)| t.iter().rev().filter(|e| e.ok))
                .find_map(|e| e.windows.iter().find(|w| matches(&w.name)).map(|w| w.unit));
            let Some(unit) = unit else { continue };
            let hash = store::meter_hash(meter);
            let meter = &assumed_meter(declared, meter);
            let kind = match (meter.unit, unit == QuotaUnit::Percent) {
                (MeterUnit::WeightedTokens, true) => Kind::Percent,
                (MeterUnit::WeightedTokens, false) => Kind::Counted,
                (MeterUnit::Requests, true) => Kind::RequestsPercent,
                (MeterUnit::Requests, false) => Kind::RequestsCounted,
            };
            let spec = Spec {
                kind,
                accounts: names.clone(),
                globs: meter.model_multiplier.keys().cloned().collect(),
                utc_offset_secs: 0,
            };
            let is_new = !stored.windows.contains_key(&meter.name);
            let mut win = match stored.windows.get(&meter.name) {
                Some(s) if s.meter_hash == hash => Win::restored(s, now),
                // The learner restarts this window anyway.
                Some(_) => continue,
                None => {
                    let first = tails.iter().filter_map(|(_, t)| t.iter().find_map(Entry::time)).min();
                    Win::fresh(first.unwrap_or(now), &hash, None)
                }
            };
            let old = win.base.prior.take();
            let mut rows = Vec::new();
            for (n, tail) in &tails {
                if !account.is_none_or(|a| a == n.as_str()) {
                    continue;
                }
                let floor = win.base.account_epochs.get(n).and_then(|e| parse(e)).map_or(win.epoch, |e| e.max(win.epoch));
                let since = old.as_ref().and_then(|p| parse(&p.through)).map_or(floor, |t| t.max(floor));
                rows.extend(rows_from(n, tail, meter, since).into_iter().filter(|r| r.start < before));
            }
            if rows.is_empty() {
                if is_new {
                    stored.windows.insert(meter.name.clone(), win.to_stored());
                    changed = true;
                }
                continue;
            }
            for r in &mut rows {
                r.class = classify::classify(r, &spec, None, STEP);
            }
            win.rows = rows;
            if !settle(&mut win, &spec, meter, now) {
                tracing::warn!(provider = entity.id, window = meter.name, "pruned rows too few to fold into the fit");
                continue;
            }
            let Some((_, fit)) = &win.last else { continue };
            let new = prior_of(&spec, fit, before);
            let merged = match &old {
                Some(o) => combine(o, &new),
                None => new,
            };
            let mut sw = stored.windows.get(&meter.name).cloned().unwrap_or_else(|| win.to_stored());
            sw.prior = Some(merged);
            stored.windows.insert(meter.name.clone(), sw);
            changed = true;
            folded += 1;
        }
        if changed {
            store::save(home, &entity.id, &stored)?;
        }
    }
    Ok(folded)
}

/// One number of a window's meter.
struct Num {
    /// The store's key.
    key: String,
    number: MeterNumber,
    p: P,
    account: Option<String>,
    /// `Some` for a capacity: `C = scale / k`.
    scale: Option<f64>,
    /// The log of the value the test compares with.
    null_log: Option<f64>,
}

impl Num {
    fn natural(&self, fit: &Fit) -> f64 {
        let v = fit.theta.get(self.p);
        self.scale.map_or(v, |s| s / v)
    }
}

/// The numbers of a window with the value each is tested against: the plugin override of a
/// weight or multiplier, else the declaration; for a capacity, the account's override, else the
/// declaration.
fn numbers(spec: &Spec, meter: &MeterDecl, ov: &WindowOverrides) -> Vec<Num> {
    let ln = |v: f64| (v.is_finite() && v > 0.0).then(|| v.ln());
    let declared_w = meter.token_weights.unwrap_or_default();
    let class_index = |c: TokenClass| TokenClass::ALL.iter().position(|x| *x == c).unwrap_or(0);
    let in_effect = |c: TokenClass| ov.plugin.weights[class_index(c)].unwrap_or_else(|| weight_of(&declared_w, c));
    let input = in_effect(TokenClass::Input);
    let mut out = Vec::new();
    if matches!(spec.kind, Kind::Percent | Kind::RequestsPercent) {
        let scale = if spec.kind == Kind::Percent { 100.0 * input } else { 100.0 };
        for (a, name) in spec.accounts.iter().enumerate() {
            let capacity = ov.accounts.get(name).and_then(|o| o.capacity).or(meter.capacity);
            out.push(Num {
                key: format!("{}@{name}", MeterNumber::Capacity),
                number: MeterNumber::Capacity,
                p: P::K(a),
                account: Some(name.clone()),
                scale: Some(scale),
                null_log: capacity.and_then(ln),
            });
        }
    }
    let classes: &[TokenClass] = match spec.kind {
        Kind::Percent => &[TokenClass::Output, TokenClass::CacheRead, TokenClass::CacheWrite],
        Kind::Counted => &TokenClass::ALL,
        _ => &[],
    };
    for (c, class) in classes.iter().enumerate() {
        let (p, null) = if spec.kind == Kind::Percent {
            (P::Rho(c), in_effect(*class) / input)
        } else {
            (P::W(c), in_effect(*class))
        };
        let number = MeterNumber::Weight(*class);
        out.push(Num { key: number.to_string(), number, p, account: None, scale: None, null_log: ln(null) });
    }
    if matches!(spec.kind, Kind::Percent | Kind::Counted) {
        for (i, (glob, factor)) in meter.model_multiplier.iter().enumerate() {
            let number = MeterNumber::Multiplier(glob.clone());
            let null = ov.plugin.multipliers.get(glob).copied().unwrap_or(*factor);
            out.push(Num { key: number.to_string(), number, p: P::Mu(i), account: None, scale: None, null_log: ln(null) });
        }
    }
    out
}

/// A parameter as the view names it, for a partner of a number that can't be separated.
fn name_of(spec: &Spec, p: P) -> String {
    match p {
        P::K(a) => format!("capacity@{}", spec.accounts.get(a).map_or("?", String::as_str)),
        P::B(_, q) => format!("outside use {:02}-{:02}", q * 4, q * 4 + 4),
        P::Rho(c) => format!("weight.{}", [TokenClass::Output, TokenClass::CacheRead, TokenClass::CacheWrite].get(c).map_or("?", |t| t.as_str())),
        P::W(c) => format!("weight.{}", TokenClass::ALL.get(c).map_or("?", |t| t.as_str())),
        P::Mu(i) => format!("multiplier.{}", spec.globs.get(i).map_or("?", String::as_str)),
    }
}

/// After a refit: each number's state, and the significant numbers to publish.
fn update_numbers(win: &mut Win, spec: &Spec, meter: &MeterDecl, ov: &WindowOverrides, now: SystemTime) {
    let Some((_, fit)) = &win.last else { return };
    let insep: BTreeMap<P, Inseparable> = separability(fit).into_iter().map(|i| (i.number, i)).collect();
    let list = numbers(spec, meter, ov);
    // Numbers tested together share the lifetime bound (R5).
    let m = list
        .iter()
        .filter(|n| n.null_log.is_some() && fit.active.contains(&n.p) && !insep.contains_key(&n.p))
        .count()
        .max(1);
    let mut rows_of: BTreeMap<&str, u64> = BTreeMap::new();
    let split = split_indices(win, spec);
    for r in win.rows.iter().filter(|r| is_evidence(r.class)) {
        if !spec.accounts.iter().position(|a| *a == r.account).is_some_and(|i| split.contains(&i)) {
            *rows_of.entry(r.account.as_str()).or_default() += 1;
        }
    }
    let mut states = BTreeMap::new();
    let mut ranges = BTreeMap::new();
    let mut published = WindowFit { relative_to_input: spec.kind == Kind::Percent, ..WindowFit::default() };
    if spec.kind == Kind::Percent {
        states.insert(MeterNumber::Weight(TokenClass::Input).to_string(), NumberState::Yardstick);
    }
    for n in &list {
        let informed = fit.active.contains(&n.p);
        let held = match win.numbers.get(&n.key) {
            Some(NumberState::Fitted { since }) => Some(*since),
            _ => None,
        };
        let relearning = match win.numbers.get(&n.key) {
            Some(NumberState::Relearning { since, .. }) => Some(*since),
            _ => None,
        };
        let state = if let Some(since) = held {
            NumberState::Fitted { since }
        } else if let Some(i) = insep.get(&n.p) {
            NumberState::NotSeparable { partner: i.partner.map_or_else(|| "the other numbers".to_owned(), |p| name_of(spec, p)) }
        } else if informed
            && let (Some(null), Some(se)) = (n.null_log, fit.se(n.p))
            && rejects(n.natural(fit).ln(), null, se, m)
        {
            NumberState::Fitted { since: now }
        } else if relearning.is_none()
            && let Some((since, reason)) = &win.restarted
        {
            NumberState::Restarted { since: *since, reason: reason.clone() }
        } else {
            let intervals = match &n.account {
                Some(a) => rows_of.get(a.as_str()).copied().unwrap_or(0),
                None => fit.rows as u64,
            };
            let half_width = if informed {
                fit.range(n.p).map_or(WIDE, |(e, lo, hi)| ((hi - lo) / 2.0 / e.abs().max(1e-300)).min(WIDE))
            } else {
                WIDE
            };
            let progress = Progress { intervals: if informed { intervals } else { 0 }, half_width };
            match relearning {
                Some(since) => NumberState::Relearning { since, progress },
                None => NumberState::Learning { progress },
            }
        };
        if matches!(state, NumberState::Fitted { .. }) && informed {
            let v = n.natural(fit);
            match &n.number {
                MeterNumber::Capacity => {
                    if let Some(a) = &n.account {
                        published.capacity.insert(a.clone(), v);
                    }
                }
                MeterNumber::Weight(c) => {
                    published.weights.insert(*c, v);
                }
                MeterNumber::Multiplier(g) => {
                    published.multipliers.insert(g.clone(), v);
                }
            }
        }
        if informed && let Some((e, lo, hi)) = fit.range(n.p) {
            // A capacity is `scale / k`: the range flips.
            let range = n.scale.map_or((e, lo, hi), |s| (s / e, s / hi, s / lo));
            ranges.insert(n.key.clone(), range);
        }
        states.insert(n.key.clone(), state);
    }
    win.ranges = ranges;
    win.numbers = states;
    win.published = published;
}

/// The learner and the cells it publishes into, shared with the poll path. Holds no reference
/// to the history or the engine, so wiring it as a history observer makes no cycle.
#[derive(Debug)]
pub struct Shared {
    home: PathBuf,
    state: Arc<ArcSwap<EngineState>>,
    fits: Arc<ArcSwap<Fits>>,
    meters: Arc<Meters>,
    learner: Mutex<Learner>,
}

impl Shared {
    pub fn new(home: &Path, state: Arc<ArcSwap<EngineState>>, fits: Arc<ArcSwap<Fits>>, meters: Arc<Meters>) -> Arc<Self> {
        Arc::new(Self { home: home.to_owned(), state, fits, meters, learner: Mutex::default() })
    }

    /// The observer to register with `History::on_entry`: learns after each good entry is
    /// durably appended, on the writer's thread (the blocking pool), never on a request.
    pub fn observer(self: &Arc<Self>) -> history::EntryObserver {
        let this = self.clone();
        Arc::new(move |provider: &str, account: &str, entry: &Entry| {
            if entry.ok {
                this.observe(provider, account);
            }
        })
    }

    /// Reads `account`'s history from the learner's cursor and learns from it.
    pub fn observe(&self, provider: &str, account: &str) {
        let st = self.state.load_full();
        let mut learner = lock(&self.learner);
        let since = learner.since(&self.home, provider, account);
        let tail = match history::read(&self.home, provider, account, since, None) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(provider, account, "fit not updated, history unreadable: {e}");
                return;
            }
        };
        let mut next = (*self.fits.load_full()).clone();
        if next.observe(&mut learner, &self.home, &st, provider, account, &tail, clock::now()) {
            self.publish(next);
        }
    }

    /// Rebuilds the fits from the history of every account at start (R11): the epoch's rows are
    /// replayed, all accounts of a plugin before its first fit.
    pub fn replay(&self) {
        let st = self.state.load_full();
        let mut learner = lock(&self.learner);
        let now = clock::now();
        let mut next = (*self.fits.load_full()).clone();
        let mut changed = false;
        for entity in st.registry.providers() {
            if entity.routing().windows.is_empty() {
                continue;
            }
            let mut tails: Vec<(String, Vec<Entry>)> = Vec::new();
            for account in st.accounts.for_provider(&entity.id) {
                let since = learner.since(&self.home, &entity.id, &account.name);
                match history::read(&self.home, &entity.id, &account.name, since, None) {
                    Ok(t) if !t.is_empty() => tails.push((account.name.clone(), t)),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(provider = entity.id, account = account.name, "fit replay skipped, history unreadable: {e}"),
                }
            }
            if tails.is_empty() {
                continue;
            }
            let refs: Vec<(&str, &[Entry])> = tails.iter().map(|(a, t)| (a.as_str(), t.as_slice())).collect();
            changed |= learner.run(&mut next, &self.home, &st, &entity.id, &refs, now);
        }
        if changed {
            self.publish(next);
        }
    }

    /// The fits and the meters built from them, swapped in. The meters follow the newest
    /// snapshot, so a reload in between is not undone.
    fn publish(&self, next: Fits) {
        self.fits.store(Arc::new(next));
        self.meters.rebuild(&self.state.load(), &self.fits.load());
    }

    /// Every window the learner holds, copied out under one brief lock (the view's read).
    pub fn snapshot_all(&self) -> BTreeMap<(String, String), WindowSnapshot> {
        lock(&self.learner).snapshot_all()
    }

    /// Whether the last save of `provider`'s fit state failed, as the view warns.
    pub fn save_warning(&self, provider: &str) -> Option<String> {
        lock(&self.learner).saves.get(provider).and_then(SaveState::warning)
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    fn meter(name: &str, length: &str, capacity: Option<f64>) -> MeterDecl {
        let cap = capacity.map_or(String::new(), |c| format!("capacity = {c}\n"));
        let text = format!("name = \"{name}\"\nlength = \"{length}\"\nunit = \"weighted_tokens\"\n{cap}");
        toml::from_str(&text).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_missing_capacity_starts_from_the_median_of_its_peers() {
        let all = [meter("a", "5h", Some(100.0)), meter("b", "5h", None), meter("c", "5h", Some(300.0)), meter("d", "7d", Some(9.0))];
        assert_eq!(assumed_meter(&all, &all[1]).capacity, Some(300.0));
        assert_eq!(assumed_meter(&all, &all[0]).capacity, Some(100.0), "declared stays declared");
        let alone = [meter("b", "5h", None)];
        assert_eq!(assumed_meter(&alone, &alone[0]).capacity, None);
    }

    #[test]
    fn counted_and_balance_windows_have_no_capacity_number() {
        let m = meter("credits", "30d", None);
        let spec = Spec { kind: Kind::Counted, accounts: vec!["a".into()], globs: vec![], utc_offset_secs: 0 };
        assert!(numbers(&spec, &m, &WindowOverrides::default()).iter().all(|n| n.number != MeterNumber::Capacity));
    }
}

#[cfg(test)]
mod outside_list_tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::super::linalg::Mat;
    use super::super::rows::Group;
    use super::*;

    fn meter() -> MeterDecl {
        toml::from_str("name = \"weekly\"\nlength = \"7d\"\nunit = \"weighted_tokens\"\n").unwrap_or_else(|e| panic!("{e}"))
    }

    fn row(start: u64, traffic: bool, y: f64, class: Class) -> Row {
        let x = if traffic { BTreeMap::from([((Group::Plain, TokenClass::Input), 1)]) } else { BTreeMap::new() };
        Row {
            account: "a".into(),
            window: "weekly".into(),
            start: UNIX_EPOCH + Duration::from_secs(start),
            end: UNIX_EPOCH + Duration::from_secs(start + 600),
            y,
            x,
            requests: u64::from(traffic),
            hours: 1.0 / 6.0,
            class,
        }
    }

    fn synthetic(n: u64) -> Vec<Row> {
        (0..n)
            .map(|i| {
                let (inp, out) = (5_000 + (i * 37 % 11) * 4_000, 1_000 + (i * 53 % 7) * 2_500);
                let noise = ((i * 7_919) % 101) as f64 / 101.0 - 0.5;
                let start = UNIX_EPOCH + Duration::from_secs(1_790_000_000 + i * 1_800);
                Row {
                    account: "a".into(),
                    window: "weekly".into(),
                    start,
                    end: start + Duration::from_secs(600),
                    y: 1e-3 * (inp as f64 + 3.0 * out as f64) + noise,
                    x: BTreeMap::from([((Group::Plain, TokenClass::Input), inp), ((Group::Plain, TokenClass::Output), out)]),
                    requests: 1,
                    hours: 1.0 / 6.0,
                    class: Class::Evidence,
                }
            })
            .collect()
    }

    #[test]
    fn a_folded_prior_keeps_the_fit_within_one_standard_error() {
        let spec = Spec { kind: Kind::Percent, accounts: vec!["a".into()], globs: vec![], utc_offset_secs: 0 };
        let mut start = Theta::neutral(1, 0);
        start.k = vec![1e-3];
        let rows = synthetic(300);
        let full = model::fit(&spec, &model::prepare(&spec, &rows), &start).expect("full fit");
        let (pruned, kept) = rows.split_at(150);
        let half = model::fit(&spec, &model::prepare(&spec, pruned), &start).expect("pruned fit");
        let prior = prior_of(&spec, &half, UNIX_EPOCH);
        let term = prior_term(&prior);
        let after = model::fit_with_prior(&spec, &model::prepare(&spec, kept), &start, Some(&term)).expect("fit with prior");
        let se = full.se(P::K(0)).expect("se");
        let moved = (after.theta.k[0].ln() - full.theta.k[0].ln()).abs();
        assert!(moved < se, "k moved {moved} against a standard error of {se}");
        // A prior from another meter (an account that is gone) is ignored.
        let other = PriorTerm { params: vec!["k@gone".into()], mean: vec![0.0], information: vec![vec![1e9]] };
        let plain = model::fit(&spec, &model::prepare(&spec, kept), &start).expect("plain");
        let ignored = model::fit_with_prior(&spec, &model::prepare(&spec, kept), &start, Some(&other)).expect("ignored");
        assert_eq!(plain.theta, ignored.theta);
    }

    #[test]
    fn combining_a_prior_with_itself_doubles_the_information_and_keeps_the_mean() {
        let p = Prior {
            through: "2026-10-06T00:00:00.000Z".into(),
            params: vec!["k@a".into(), "rho.output".into()],
            mean: vec![-6.0, 1.0],
            information: vec![vec![4.0, 1.0], vec![1.0, 3.0]],
        };
        let both = combine(&p, &p);
        assert_eq!(both.params, p.params);
        assert!((both.information[0][0] - 8.0).abs() < 1e-9 && (both.information[0][1] - 2.0).abs() < 1e-9);
        assert!(both.mean.iter().zip(&p.mean).all(|(a, b)| (a - b).abs() < 1e-6), "{:?}", both.mean);
    }

    fn win(rows: Vec<Row>, spec: &Spec) -> Win {
        let mut w = Win::fresh(UNIX_EPOCH, "h", None);
        w.rows = rows;
        let fit = Fit {
            theta: Theta::neutral(1, 0),
            active: vec![P::K(0)],
            cov: Mat::identity(1),
            info: Mat::identity(1),
            rows: 10,
            rss: 0.0,
            sigma_e2: 0.0,
            converged: true,
        };
        w.last = Some((spec.clone(), fit));
        w
    }

    #[test]
    fn idle_and_busy_outside_rows_are_each_listed_once_across_refits_and_replays() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let spec = Spec { kind: Kind::Percent, accounts: vec!["a".into()], globs: vec![], utc_offset_secs: 0 };
        let m = meter();
        let now = UNIX_EPOCH + Duration::from_secs(100_000);
        let until = UNIX_EPOCH + Duration::from_secs(50_000);
        let rows = vec![row(1000, false, 3.0, Class::Outside), row(2000, true, 500.0, Class::OutsideProvisional { until })];
        let count = || outside::read(home, "p", "a", None, None).unwrap();

        let mut w = win(rows, &spec);
        // The busy row is provisional: only the idle row is listed, however often we refit.
        for _ in 0..3 {
            list_outside_rows(&mut w, home, "p", &spec, &m, QuotaUnit::Percent, now);
        }
        let got = count();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].ty, got[0].amount), (outside::OutsideType::Idle, Some(3.0)));

        // It turns final: now it is listed, once.
        w.rows[1].class = Class::Outside;
        for _ in 0..3 {
            list_outside_rows(&mut w, home, "p", &spec, &m, QuotaUnit::Percent, now);
        }
        let got = count();
        assert_eq!(got.len(), 2);
        let busy = got.iter().find(|e| e.ty == outside::OutsideType::Busy).expect("busy listed");
        assert!(busy.amount.is_some_and(|a| a > 0.0), "{busy:?}");

        // A fresh learner replaying the same history lists nothing more.
        let mut again = win(w.rows.clone(), &spec);
        for _ in 0..2 {
            list_outside_rows(&mut again, home, "p", &spec, &m, QuotaUnit::Percent, now);
        }
        assert_eq!(count().len(), 2);
    }
}
