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
//! The value tested against is the declared value. Operator overrides of weights and
//! multipliers are not wired into the engine yet (`Meters::rebuild` passes none), so there is
//! nothing else in effect to test against.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use arc_swap::ArcSwap;
use nullrouter_registry::schema::{MeterDecl, MeterUnit, QuotaUnit, TokenWeights, glob_match};

use super::classify::{self, Inseparable, is_evidence, reclassify_epoch, separability};
use super::outside;
use super::model::{self, Fit, Kind, P, Spec, Theta};
use super::rows::{Class, Row, rows_from};
use super::split::{self, test_splits};
use super::store::{self, Loaded, Restart, SaveState, StoredFit, StoredNumber, StoredWindow};
use super::test::rejects;
use super::{Fits, MeterNumber, Meters, NumberState, Progress, TokenClass, WindowFit};
use crate::clock;
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
        }
    }

    fn restored(s: &StoredWindow, now: SystemTime) -> Self {
        let mut w = Self::fresh(parse(&s.epoch).unwrap_or(now), &s.meter_hash, None);
        w.restarted = s.restarted.as_ref().and_then(|r| Some((parse(&r.at)?, r.reason.clone())));
        for (key, n) in &s.numbers {
            if n.state == "fitted"
                && let Some(since) = n.since.as_deref().and_then(parse)
            {
                w.numbers.insert(key.clone(), NumberState::Fitted { since });
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
        NumberState::Learning { .. } | NumberState::Relearning { .. } | NumberState::Restarted { .. } => {
            ("learning", None)
        }
        NumberState::Yardstick | NumberState::NotReported | NumberState::NotFitted(_) => return None,
    };
    Some(StoredNumber { state: state.to_owned(), since })
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
            self.unfitted.insert((provider.to_owned(), a.name.clone()), "no quota reports (or pay-as-you-go)".to_owned());
        }
        let accounts: Vec<String> = fitted.iter().map(|a| a.name.clone()).collect();
        self.run_declared(fits, home, provider, entity.routing().windows, &accounts, tails, now)
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
        self.run_declared(fits, home, provider, declared, accounts, tails, now)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_declared(
        &mut self,
        fits: &mut Fits,
        home: &Path,
        provider: &str,
        declared: &[MeterDecl],
        accounts: &[String],
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
            self.window(home, provider, declared, meter, &accounts, tails, now);
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
    fn window(&mut self, home: &Path, provider: &str, declared: &[MeterDecl], meter: &MeterDecl, accounts: &[String], tails: &[(&str, &[Entry])], now: SystemTime) {
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
            let since = win.cursors.get(*account).copied().unwrap_or(win.epoch);
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
        refit(win, &spec, meter, now);
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
fn refit(win: &mut Win, spec: &Spec, meter: &MeterDecl, now: SystemTime) {
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
    update_numbers(win, spec, meter, now);
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
        let Some(fit) = model::fit(spec, &pool, &start) else { return fitted };
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

fn numbers(spec: &Spec, meter: &MeterDecl) -> Vec<Num> {
    let ln = |v: f64| (v.is_finite() && v > 0.0).then(|| v.ln());
    let w = meter.token_weights.unwrap_or_default();
    let mut out = Vec::new();
    if matches!(spec.kind, Kind::Percent | Kind::RequestsPercent) {
        let scale = if spec.kind == Kind::Percent { 100.0 * w.input } else { 100.0 };
        for (a, name) in spec.accounts.iter().enumerate() {
            out.push(Num {
                key: format!("{}@{name}", MeterNumber::Capacity),
                number: MeterNumber::Capacity,
                p: P::K(a),
                account: Some(name.clone()),
                scale: Some(scale),
                null_log: meter.capacity.and_then(ln),
            });
        }
    }
    let classes: &[TokenClass] = match spec.kind {
        Kind::Percent => &[TokenClass::Output, TokenClass::CacheRead, TokenClass::CacheWrite],
        Kind::Counted => &TokenClass::ALL,
        _ => &[],
    };
    for (c, class) in classes.iter().enumerate() {
        let (p, declared) = if spec.kind == Kind::Percent {
            (P::Rho(c), weight_of(&w, *class) / w.input)
        } else {
            (P::W(c), weight_of(&w, *class))
        };
        let number = MeterNumber::Weight(*class);
        out.push(Num { key: number.to_string(), number, p, account: None, scale: None, null_log: ln(declared) });
    }
    if matches!(spec.kind, Kind::Percent | Kind::Counted) {
        for (i, (glob, factor)) in meter.model_multiplier.iter().enumerate() {
            let number = MeterNumber::Multiplier(glob.clone());
            out.push(Num { key: number.to_string(), number, p: P::Mu(i), account: None, scale: None, null_log: ln(*factor) });
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
fn update_numbers(win: &mut Win, spec: &Spec, meter: &MeterDecl, now: SystemTime) {
    let Some((_, fit)) = &win.last else { return };
    let insep: BTreeMap<P, Inseparable> = separability(fit).into_iter().map(|i| (i.number, i)).collect();
    let list = numbers(spec, meter);
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
        let state = if let Some(since) = held {
            NumberState::Fitted { since }
        } else if let Some(i) = insep.get(&n.p) {
            NumberState::NotSeparable { partner: i.partner.map_or_else(|| "the other numbers".to_owned(), |p| name_of(spec, p)) }
        } else if informed
            && let (Some(null), Some(se)) = (n.null_log, fit.se(n.p))
            && rejects(n.natural(fit).ln(), null, se, m)
        {
            NumberState::Fitted { since: now }
        } else if let Some((since, reason)) = &win.restarted {
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
            NumberState::Learning { progress: Progress { intervals: if informed { intervals } else { 0 }, half_width } }
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
        assert!(numbers(&spec, &m).iter().all(|n| n.number != MeterNumber::Capacity));
    }
}
